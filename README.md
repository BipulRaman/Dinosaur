# Big File Viewer

A native desktop viewer for **very large** `CSV`, `TSV`, `JSON` (newline-delimited), and `TXT` files. It opens and scrolls **50 GB+** files smoothly because it never loads the whole file into memory.

## Why it stays fast on huge files

| Technique | What it does |
|-----------|--------------|
| **Memory-mapped I/O** (`memmap2`) | The file is mapped into the address space; the OS pages in only what's touched. The whole file is never read into RAM. |
| **Sparse parallel line index** (`memchr` + `rayon`) | Newlines are scanned in parallel across all CPU cores. Only one byte offset is stored every 4096 lines, so a 50 GB file with ~500 M lines costs a few MB of index instead of ~4 GB. Any line is located in O(1) by jumping to the nearest checkpoint and scanning forward. |
| **Virtualized table** (`egui_extras`) | Only the rows currently visible in the viewport (~50) are drawn. |
| **Lazy parsing + cache** | CSV/JSON parsing runs only on visible rows, with a small cache. Work is bounded by viewport size, not file size. |
| **Background indexing** | Indexing runs on a worker thread with a live progress bar, so the UI stays responsive. |

## Supported formats

- **CSV** (`.csv`) — comma-separated, first line treated as header.
- **TSV** (`.tsv`) — tab-separated, first line treated as header.
- **JSON lines** (`.json`, `.ndjson`, `.jsonl`) — one JSON object per line (NDJSON). Object keys become columns.
- **Text** (`.txt`, `.log`, anything else) — one line per row.

> Note: A single giant JSON array/object (not line-delimited) is intentionally not supported in streaming mode — use NDJSON for large JSON data.

## Requirements

- Rust toolchain (tested with `cargo`/`rustc` 1.94).

## Build & run

```powershell
# Release build is strongly recommended for large files
cargo run --release
```

Then click **📂 Open** and pick a file.

## Usage

- **📂 Open** — choose a file via the native file dialog.
- **Go to row** — type a row number and press Enter to jump.
- **Status bar** — shows format, row count, column count, and file size.
- Columns are **resizable**; long cell values are clipped and shown in full on hover.

## Project structure

```
src/
  main.rs     GUI (eframe/egui), state machine, virtualized table, background loader
  index.rs    Sparse byte-offset line index (mmap + memchr + rayon)
  format.rs   Format detection, header extraction, per-line parsing
Cargo.toml    Dependencies and release profile (LTO enabled)
```

## How it works (flow)

1. Pick a file → it is memory-mapped.
2. A worker thread scans newlines in parallel and builds the sparse index (progress bar shown).
3. The header (CSV/TSV first line or NDJSON keys) is extracted.
4. The virtualized table renders; as you scroll, only visible rows are located via the index and parsed.

## Tuning

- `DEFAULT_SAMPLE` in [src/index.rs](src/index.rs) controls index density (lines per checkpoint). Larger = less RAM but slightly slower per-line lookups; smaller = more RAM, faster lookups. The default of 4096 is a good balance.

## License

MIT (add a LICENSE file if you intend to distribute).
