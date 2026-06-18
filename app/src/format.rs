//! File-format detection and per-line parsing.
//!
//! Only the visible rows are ever parsed, so the cost of parsing is bounded by
//! the size of the viewport rather than the size of the file.

use std::path::Path;

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// Comma-separated values; every line is data.
    Csv,
    /// Tab-separated values; every line is data.
    Tsv,
    /// Newline-delimited JSON (one JSON object per line).
    Ndjson,
    /// Plain text, one column per line.
    Txt,
}

impl Format {
    pub fn from_path(path: &Path) -> Format {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("csv") => Format::Csv,
            Some("tsv") => Format::Tsv,
            Some("json") | Some("ndjson") | Some("jsonl") => Format::Ndjson,
            _ => Format::Txt,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
            Format::Ndjson => "JSON (lines)",
            Format::Txt => "Text",
        }
    }

    fn delimiter(self) -> u8 {
        match self {
            Format::Tsv => b'\t',
            _ => b',',
        }
    }

    /// Whether the first physical line is a header that should not be shown as
    /// a data row. No format treats the first line as a header; every physical
    /// line is shown as data.
    pub fn has_header(self) -> bool {
        false
    }

    /// Whether record boundaries must honour double-quoted fields. Only CSV
    /// does: an RFC 4180 quoted cell may span several physical lines, so a bare
    /// `\n` does not always end a record. Tab-separated values follow the IANA
    /// convention where fields never contain raw tabs or newlines (those are
    /// backslash-escaped), so a `"` is ordinary data and every `\n` ends a row.
    /// Treating TSV as quoted would let a single stray `"` (e.g. `2" pipe`)
    /// swallow the following rows. NDJSON and plain text also break on every
    /// newline.
    pub fn quote_aware(self) -> bool {
        matches!(self, Format::Csv)
    }
}

/// Strip a leading UTF-8 byte-order mark, if present. Excel and many Windows
/// tools prefix exported files with `EF BB BF`; left in place it corrupts the
/// first cell of the first row and breaks JSON parsing of the first line.
fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// Split a single delimited line into fields. `quoting` enables RFC 4180
/// double-quote handling (CSV); when it is off (TSV) a `"` is literal data.
fn parse_delimited(bytes: &[u8], delim: u8, quoting: bool) -> Vec<String> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delim)
        .quoting(quoting)
        .has_headers(false)
        .flexible(true)
        .from_reader(bytes);
    if let Some(Ok(record)) = reader.records().next() {
        record.iter().map(|f| f.to_string()).collect()
    } else {
        vec![String::from_utf8_lossy(bytes).into_owned()]
    }
}

/// Compute the column headers for a file given its format and first line.
pub fn headers(format: Format, first_line: &[u8]) -> Vec<String> {
    let first_line = strip_bom(first_line);
    match format {
        Format::Csv | Format::Tsv => {
            // The first line is treated as data, so use generic column names
            // sized to the number of fields in that line.
            let n = parse_delimited(first_line, format.delimiter(), format.quote_aware())
                .len()
                .max(1);
            (1..=n).map(|i| format!("Column {i}")).collect()
        }
        Format::Ndjson => {
            if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(first_line) {
                map.keys().cloned().collect()
            } else {
                vec!["value".to_string()]
            }
        }
        Format::Txt => vec!["line".to_string()],
    }
}

/// Parse a single data line into cells aligned with `headers`.
pub fn parse_row(format: Format, headers: &[String], bytes: &[u8]) -> Vec<String> {
    let bytes = strip_bom(bytes);
    match format {
        Format::Csv | Format::Tsv => {
            parse_delimited(bytes, format.delimiter(), format.quote_aware())
        }
        Format::Ndjson => match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(map)) => headers
                .iter()
                .map(|k| match map.get(k) {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                })
                .collect(),
            Ok(other) => vec![other.to_string()],
            Err(_) => vec![String::from_utf8_lossy(bytes).into_owned()],
        },
        Format::Txt => vec![String::from_utf8_lossy(bytes).into_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(fmt: Format, line: &str) -> Vec<String> {
        let headers = headers(fmt, line.as_bytes());
        parse_row(fmt, &headers, line.as_bytes())
    }

    #[test]
    fn tsv_treats_quote_as_literal() {
        // A stray double quote must not turn the rest of the cell into a quoted
        // field (which previously also merged following rows in the index).
        let cells = row(Format::Tsv, "2\" pipe\tin stock");
        assert_eq!(cells, vec!["2\" pipe".to_string(), "in stock".to_string()]);
    }

    #[test]
    fn tsv_keeps_balanced_quotes_verbatim() {
        let cells = row(Format::Tsv, "she said \"hi\"\tok");
        assert_eq!(
            cells,
            vec!["she said \"hi\"".to_string(), "ok".to_string()]
        );
    }

    #[test]
    fn csv_still_honours_quoting() {
        // CSV keeps RFC 4180 behaviour: a quoted field may contain the delimiter.
        let cells = row(Format::Csv, "\"a,b\",c");
        assert_eq!(cells, vec!["a,b".to_string(), "c".to_string()]);
    }

    #[test]
    fn bom_is_stripped_from_first_cell() {
        let line = "\u{feff}id,name";
        let cells = row(Format::Csv, line);
        assert_eq!(cells, vec!["id".to_string(), "name".to_string()]);
    }

    #[test]
    fn bom_does_not_break_ndjson() {
        let line = "\u{feff}{\"a\":1}";
        let headers = headers(Format::Ndjson, line.as_bytes());
        assert_eq!(headers, vec!["a".to_string()]);
        let cells = parse_row(Format::Ndjson, &headers, line.as_bytes());
        assert_eq!(cells, vec!["1".to_string()]);
    }

    #[test]
    fn tsv_is_not_quote_aware() {
        assert!(!Format::Tsv.quote_aware());
        assert!(Format::Csv.quote_aware());
        assert!(!Format::Ndjson.quote_aware());
        assert!(!Format::Txt.quote_aware());
    }
}

