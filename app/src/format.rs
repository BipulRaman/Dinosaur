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
}

/// Split a single delimited line into fields, honouring quoted values.
fn parse_delimited(bytes: &[u8], delim: u8) -> Vec<String> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delim)
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
    match format {
        Format::Csv | Format::Tsv => {
            // The first line is treated as data, so use generic column names
            // sized to the number of fields in that line.
            let n = parse_delimited(first_line, format.delimiter()).len().max(1);
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
    match format {
        Format::Csv | Format::Tsv => parse_delimited(bytes, format.delimiter()),
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
