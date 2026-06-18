//! Sparse byte-offset line index for very large files.
//!
//! Instead of storing the offset of *every* line (which for a 50 GB file with
//! ~500M lines would cost ~4 GB of RAM), we store a checkpoint only every
//! `sample` lines. To locate an arbitrary line we jump to the nearest
//! checkpoint and scan forward at most `sample` newlines using `memchr`
//! (which runs at multiple GB/s). This keeps the index in the low-MB range
//! while line lookups stay effectively O(1).

use std::sync::atomic::{AtomicU64, Ordering};

use memchr::{memchr, memchr_iter};
use rayon::prelude::*;

/// Number of lines between stored checkpoints.
pub const DEFAULT_SAMPLE: u64 = 4096;

pub struct LineIndex {
    /// `checkpoints[k]` is the byte offset where line `k * sample` begins.
    checkpoints: Vec<u64>,
    sample: u64,
    pub total_lines: u64,
    pub file_len: u64,
}

impl LineIndex {
    /// Build a sparse line index over `data`.
    ///
    /// `progress` is updated with the number of bytes scanned so the UI can
    /// render a progress bar while indexing a huge file on a background thread.
    pub fn build(data: &[u8], sample: u64, progress: &AtomicU64) -> Self {
        let len = data.len();
        if len == 0 {
            return LineIndex {
                checkpoints: vec![0],
                sample,
                total_lines: 0,
                file_len: 0,
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
        while pos < offset {
            match memchr(b'\n', &data[pos..offset]) {
                Some(p) => {
                    line += 1;
                    pos += p + 1;
                }
                None => break,
            }
        }
        line.min(self.total_lines.saturating_sub(1))
    }
}
