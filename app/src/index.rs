//! Sparse byte-offset line index for very large files.
//!
//! Instead of storing the offset of *every* line (which for a 50 GB file with
//! ~500M lines would cost ~4 GB of RAM), we store a checkpoint only every
//! `sample` lines. To locate an arbitrary line we jump to the nearest
//! checkpoint and scan forward at most `sample` newlines using `memchr`
//! (which runs at multiple GB/s). This keeps the index in the low-MB range
//! while line lookups stay effectively O(1).

use std::sync::atomic::{AtomicU64, Ordering};

use memchr::{memchr, memchr2, memchr_iter};
use rayon::prelude::*;

/// Number of lines between stored checkpoints.
pub const DEFAULT_SAMPLE: u64 = 4096;

pub struct LineIndex {
    /// `checkpoints[k]` is the byte offset where line `k * sample` begins.
    checkpoints: Vec<u64>,
    sample: u64,
    pub total_lines: u64,
    pub file_len: u64,
    /// When set, a `\n` only ends a record if it is not inside a double-quoted
    /// field. Used for CSV/TSV, where a quoted cell may span several physical
    /// lines (RFC 4180). When clear, every `\n` is a line break (NDJSON/TXT).
    quote_aware: bool,
}

/// Starting at `start` — which must be the beginning of a logical record and
/// therefore outside any quoted field — return the byte index of the `\n` that
/// terminates the record, or `data.len()` if the record runs to end of file.
/// Newlines that appear inside a `"`-quoted field are skipped. RFC 4180
/// escaped quotes (`""`) are handled naturally because each `"` toggles the
/// in-field state and the two characters are always adjacent.
fn record_end(data: &[u8], start: usize) -> usize {
    let mut pos = start;
    let mut inside = false;
    while pos < data.len() {
        match memchr2(b'"', b'\n', &data[pos..]) {
            Some(rel) => {
                let abs = pos + rel;
                if data[abs] == b'"' {
                    inside = !inside;
                    pos = abs + 1;
                } else if inside {
                    pos = abs + 1; // embedded newline inside a quoted field
                } else {
                    return abs; // genuine record terminator
                }
            }
            None => return data.len(),
        }
    }
    data.len()
}

impl LineIndex {
    /// Build a sparse line index over `data`.
    ///
    /// `progress` is updated with the number of bytes scanned so the UI can
    /// render a progress bar while indexing a huge file on a background thread.
    pub fn build(data: &[u8], sample: u64, quote_aware: bool, progress: &AtomicU64) -> Self {
        let len = data.len();
        if len == 0 {
            return LineIndex {
                checkpoints: vec![0],
                sample,
                total_lines: 0,
                file_len: 0,
                quote_aware,
            };
        }

        // Split the file into one chunk per CPU thread.
        let threads = rayon::current_num_threads().max(1);
        let chunk = (len / threads).max(1);
        let bounds: Vec<(usize, usize)> = (0..threads)
            .map(|i| {
                let start = (i * chunk).min(len);
                let end = if i == threads - 1 {
                    len
                } else {
                    ((i + 1) * chunk).min(len)
                };
                (start, end)
            })
            .filter(|(s, e)| s < e)
            .collect();

        if quote_aware {
            return Self::build_quote_aware(data, sample, &bounds, progress);
        }

        // Pass 1: count newlines per chunk (drives the progress bar).
        let counts: Vec<u64> = bounds
            .par_iter()
            .map(|&(s, e)| {
                let c = memchr_iter(b'\n', &data[s..e]).count() as u64;
                progress.fetch_add((e - s) as u64, Ordering::Relaxed);
                c
            })
            .collect();

        // Global index of the first newline contained in each chunk.
        let mut newline_base = vec![0u64; bounds.len()];
        let mut running = 0u64;
        for i in 0..bounds.len() {
            newline_base[i] = running;
            running += counts[i];
        }
        let total_newlines = running;

        // Pass 2: record the start offset of every `sample`-th line.
        // The newline with global index `gi` terminates line `gi`, so line
        // `gi + 1` starts at `pos + 1`. We keep the offset whenever
        // `(gi + 1) % sample == 0`.
        let per_chunk: Vec<Vec<u64>> = bounds
            .par_iter()
            .enumerate()
            .map(|(ci, &(s, e))| {
                let base = newline_base[ci];
                let mut local = 0u64;
                let mut out = Vec::new();
                for pos in memchr_iter(b'\n', &data[s..e]) {
                    let line_no = base + local + 1;
                    local += 1;
                    if line_no % sample == 0 {
                        out.push((s + pos + 1) as u64);
                    }
                }
                out
            })
            .collect();

        let mut checkpoints = Vec::with_capacity((total_newlines / sample) as usize + 1);
        checkpoints.push(0u64); // line 0 always starts at offset 0
        for cv in per_chunk {
            checkpoints.extend(cv);
        }

        // If the file does not end in a newline, the trailing bytes form one
        // extra (unterminated) line.
        let has_trailing = data[len - 1] != b'\n';
        let total_lines = total_newlines + has_trailing as u64;

        LineIndex {
            checkpoints,
            sample,
            total_lines,
            file_len: len as u64,
            quote_aware: false,
        }
    }

    /// Quote-aware variant of [`build`](Self::build) for CSV/TSV. A `\n` ends a
    /// record only when it is *not* inside a double-quoted field, so cells that
    /// contain embedded newlines stay on a single logical row.
    ///
    /// Quote parity is additive, so it can still be computed in parallel. The
    /// catch is that whether a chunk *starts* inside a quoted field depends on
    /// the quote parity of every preceding chunk, which we don't know until all
    /// chunks have been counted. To avoid a third pass we count, in one pass,
    /// the record-terminating newlines for *both* possible entry parities
    /// (`term_even` if the chunk starts outside quotes, `term_odd` if inside)
    /// together with the chunk's quote count. After prefix-summing the quote
    /// counts we know each chunk's true entry parity and simply pick the
    /// matching terminator count. A second pass then emits the checkpoints.
    fn build_quote_aware(
        data: &[u8],
        sample: u64,
        bounds: &[(usize, usize)],
        progress: &AtomicU64,
    ) -> Self {
        let len = data.len();

        // Pass 1: per chunk, count quotes plus the record-terminating newlines
        // for each possible entry parity. A newline at running in-chunk quote
        // parity `p` is a terminator when the chunk's entry parity equals `p`.
        struct ChunkCount {
            quotes: u64,
            term_even: u64,
            term_odd: u64,
        }
        let counts: Vec<ChunkCount> = bounds
            .par_iter()
            .map(|&(s, e)| {
                let mut quotes = 0u64;
                let mut term_even = 0u64;
                let mut term_odd = 0u64;
                let mut qparity = 0u8; // running (#quotes seen in chunk) & 1
                let mut pos = s;
                while pos < e {
                    match memchr2(b'"', b'\n', &data[pos..e]) {
                        Some(rel) => {
                            let abs = pos + rel;
                            if data[abs] == b'"' {
                                quotes += 1;
                                qparity ^= 1;
                            } else if qparity == 0 {
                                term_even += 1;
                            } else {
                                term_odd += 1;
                            }
                            pos = abs + 1;
                        }
                        None => break,
                    }
                }
                progress.fetch_add(((e - s) as u64) / 2, Ordering::Relaxed);
                ChunkCount {
                    quotes,
                    term_even,
                    term_odd,
                }
            })
            .collect();

        // Prefix-sum quotes to get the entry parity of each chunk, then pick the
        // matching terminator count to get the running line base.
        let mut quote_base = vec![0u64; bounds.len()];
        let mut rec_base = vec![0u64; bounds.len()];
        let mut q_run = 0u64;
        let mut r_run = 0u64;
        for i in 0..bounds.len() {
            quote_base[i] = q_run;
            rec_base[i] = r_run;
            let entry_inside = (q_run & 1) == 1;
            r_run += if entry_inside {
                counts[i].term_odd
            } else {
                counts[i].term_even
            };
            q_run += counts[i].quotes;
        }
        let total_quotes = q_run;
        let total_terminators = r_run;

        // Pass 2: record the start offset of every `sample`-th logical line.
        let per_chunk: Vec<Vec<u64>> = bounds
            .par_iter()
            .enumerate()
            .map(|(ci, &(s, e))| {
                let mut inside = (quote_base[ci] & 1) == 1;
                let mut pos = s;
                let mut local = 0u64;
                let mut out = Vec::new();
                while pos < e {
                    match memchr2(b'"', b'\n', &data[pos..e]) {
                        Some(rel) => {
                            let abs = pos + rel;
                            if data[abs] == b'"' {
                                inside = !inside;
                            } else if !inside {
                                // This newline terminates record `rec_base + local`,
                                // so the next logical line starts at `abs + 1`.
                                let line_no = rec_base[ci] + local + 1;
                                local += 1;
                                if line_no % sample == 0 {
                                    out.push((abs + 1) as u64);
                                }
                            }
                            pos = abs + 1;
                        }
                        None => break,
                    }
                }
                progress.fetch_add(((e - s) as u64) / 2, Ordering::Relaxed);
                out
            })
            .collect();

        let mut checkpoints = Vec::with_capacity((total_terminators / sample) as usize + 1);
        checkpoints.push(0u64); // line 0 always starts at offset 0
        for cv in per_chunk {
            checkpoints.extend(cv);
        }

        // The file ends with a complete record only when its last byte is a
        // newline *and* that newline is not inside an (unterminated) quoted
        // field. Otherwise the trailing bytes form one extra logical line.
        let ends_clean = data[len - 1] == b'\n' && (total_quotes & 1) == 0;
        let total_lines = total_terminators + (!ends_clean as u64);

        LineIndex {
            checkpoints,
            sample,
            total_lines,
            file_len: len as u64,
            quote_aware: true,
        }
    }

    /// Return the `[start, end)` byte range of line `line` (end excludes the
    /// trailing newline). Returns an empty range if `line` is out of bounds.
    pub fn line_range(&self, data: &[u8], line: u64) -> (usize, usize) {
        if line >= self.total_lines {
            return (data.len(), data.len());
        }
        let ci = (line / self.sample) as usize;
        let base_line = ci as u64 * self.sample;
        let mut start = self.checkpoints[ci] as usize;
        let mut skip = line - base_line;

        if self.quote_aware {
            while skip > 0 {
                let e = record_end(data, start);
                if e >= data.len() {
                    return (data.len(), data.len());
                }
                start = e + 1;
                skip -= 1;
            }
            let end = record_end(data, start);
            return (start, end);
        }

        while skip > 0 {
            match memchr(b'\n', &data[start..]) {
                Some(p) => {
                    start += p + 1;
                    skip -= 1;
                }
                None => {
                    return (data.len(), data.len());
                }
            }
        }

        let end = match memchr(b'\n', &data[start..]) {
            Some(p) => start + p,
            None => data.len(),
        };
        (start, end)
    }

    /// Borrow the raw bytes of `line`, with a trailing `\r` trimmed (CRLF).
    pub fn line_bytes<'a>(&self, data: &'a [u8], line: u64) -> &'a [u8] {
        let (s, e) = self.line_range(data, line);
        let mut slice = &data[s..e];
        if slice.last() == Some(&b'\r') {
            slice = &slice[..slice.len() - 1];
        }
        slice
    }

    /// Byte offset where `line` begins. Clamped to the end of file.
    pub fn line_start(&self, data: &[u8], line: u64) -> usize {
        self.line_range(data, line).0
    }

    /// Map a byte offset back to the line number that contains it. Used by the
    /// search feature to convert a byte match position into a row index.
    pub fn line_at_offset(&self, data: &[u8], offset: usize) -> u64 {
        let offset = offset.min(data.len());
        // Greatest checkpoint whose offset is <= `offset`.
        let ci = match self.checkpoints.binary_search(&(offset as u64)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let mut line = ci as u64 * self.sample;
        let mut pos = self.checkpoints[ci] as usize;
        if self.quote_aware {
            loop {
                let e = record_end(data, pos);
                if offset <= e || e >= data.len() {
                    break;
                }
                line += 1;
                pos = e + 1;
            }
        } else {
            while pos < offset {
                match memchr(b'\n', &data[pos..offset]) {
                    Some(p) => {
                        line += 1;
                        pos += p + 1;
                    }
                    None => break,
                }
            }
        }
        line.min(self.total_lines.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(data: &[u8], quote_aware: bool) -> LineIndex {
        // Use a tiny sample so checkpoint logic is exercised even on small input.
        LineIndex::build(data, 2, quote_aware, &AtomicU64::new(0))
    }

    fn lines(idx: &LineIndex, data: &[u8]) -> Vec<String> {
        (0..idx.total_lines)
            .map(|i| String::from_utf8_lossy(idx.line_bytes(data, i)).into_owned())
            .collect()
    }

    #[test]
    fn quoted_embedded_newline_is_one_row() {
        let data = b"id,note\n1,\"hello\nworld\"\n2,plain\n";
        let idx = build(data, true);
        assert_eq!(idx.total_lines, 3);
        assert_eq!(
            lines(&idx, data),
            vec!["id,note", "1,\"hello\nworld\"", "2,plain"]
        );
    }

    #[test]
    fn escaped_quotes_and_multiple_embedded_newlines() {
        // A quoted cell containing escaped quotes ("") and two newlines.
        let data = b"a,\"x\"\"y\nz\nw\",b\nlast,row\n";
        let idx = build(data, true);
        assert_eq!(idx.total_lines, 2);
        assert_eq!(
            lines(&idx, data),
            vec!["a,\"x\"\"y\nz\nw\",b", "last,row"]
        );
    }

    #[test]
    fn crlf_quoted_newline() {
        let data = b"1,\"two\r\nlines\"\r\n2,ok\r\n";
        let idx = build(data, true);
        assert_eq!(idx.total_lines, 2);
        // line_bytes trims the terminating CR; the embedded CRLF is preserved.
        assert_eq!(
            lines(&idx, data),
            vec!["1,\"two\r\nlines\"", "2,ok"]
        );
    }

    #[test]
    fn unterminated_quote_at_eof_is_one_row() {
        let data = b"1,ok\n2,\"open\nstill open\n";
        let idx = build(data, true);
        assert_eq!(idx.total_lines, 2);
        assert_eq!(
            lines(&idx, data),
            vec!["1,ok", "2,\"open\nstill open\n"]
        );
    }

    #[test]
    fn plain_mode_splits_every_newline() {
        let data = b"1,\"hello\nworld\"\n2,plain\n";
        let idx = build(data, false);
        assert_eq!(idx.total_lines, 3);
        assert_eq!(
            lines(&idx, data),
            vec!["1,\"hello", "world\"", "2,plain"]
        );
    }

    #[test]
    fn no_trailing_newline() {
        let data = b"a,b\n1,\"x\ny\"";
        let idx = build(data, true);
        assert_eq!(idx.total_lines, 2);
        assert_eq!(lines(&idx, data), vec!["a,b", "1,\"x\ny\""]);
    }

    #[test]
    fn line_at_offset_maps_into_quoted_record() {
        let data = b"a,b\n1,\"x\ny\"\n2,z\n";
        let idx = build(data, true);
        // Offset of the embedded newline inside the quoted cell (row 1).
        let embedded_nl = data.iter().position(|&b| b == b'\n').unwrap();
        let embedded_nl = embedded_nl
            + 1
            + data[embedded_nl + 1..].iter().position(|&b| b == b'\n').unwrap();
        assert_eq!(idx.line_at_offset(data, embedded_nl), 1);
        // Round-trip: start of each row maps back to that row.
        for r in 0..idx.total_lines {
            let s = idx.line_start(data, r);
            assert_eq!(idx.line_at_offset(data, s), r);
        }
    }
}
