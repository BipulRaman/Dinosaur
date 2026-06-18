<div align="center">

<img src="app/assets/logo.svg" width="120" alt="Dinosaur logo" />

# Dinosaur

**A native desktop viewer for _very large_ data files.**

Open and scroll **huge** `CSV`, `TSV`, `JSON` (newline-delimited) and `TXT`
files instantly — tested on files **up to 170 GB**, with no known upper limit —
because Dinosaur never loads the whole file into memory.

[![Release](https://img.shields.io/github/v/release/BipulRaman/Dinosaur?label=download&logo=github)](https://github.com/BipulRaman/Dinosaur/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)

[**Website**](https://bipulraman.github.io/Dinosaur/) · [**Download**](https://github.com/BipulRaman/Dinosaur/releases/latest) · [**Report an issue**](https://github.com/BipulRaman/Dinosaur/issues)

</div>

---

## Download

Grab the latest **portable ZIP** from the
[Releases page](https://github.com/BipulRaman/Dinosaur/releases/latest),
unzip it, and run `Dinosaur.exe`. No installation, no admin rights.

> The download bundles a CPU software renderer (Mesa llvmpipe), so Dinosaur runs
> even on machines and virtual machines without a dedicated GPU. Keep the
> `.dll` files next to `Dinosaur.exe`.

You can also open files directly from the command line or by dragging them onto
the window:

```powershell
Dinosaur.exe path\to\huge.csv
```

## Features

- 📂 **Opens huge files instantly** — memory-mapped, never fully loaded into RAM (tested up to 170 GB, no known upper limit).
- 📊 **Spreadsheet-style grid** — clean Google-Sheets-like typography, gridlines, row/column headers.
- 🖱️ **Cell selection & copy** — click or drag to select a range, right-click or `Ctrl+C` to copy as TSV.
- 🔎 **Fast search** — find text forward/backward with optional case sensitivity.
- 🔢 **Go to row** — jump to any row number in a billion-row file in O(1).
- 🧩 **Multiple formats** — CSV, TSV, NDJSON, and plain text/log.
- 🖥️ **Runs anywhere** — works on GPU-less machines and cloud VMs.

## Why it stays fast on huge files

| Technique | What it does |
|-----------|--------------|
| **Memory-mapped I/O** (`memmap2`) | The file is mapped into the address space; the OS pages in only what's touched. The whole file is never read into RAM. |
| **Sparse parallel line index** (`memchr` + `rayon`) | Newlines are scanned in parallel across all CPU cores. Only one byte offset is stored every 4096 lines, so a 50 GB file with ~500 M lines costs a few MB of index instead of ~4 GB. Any line is located in O(1) by jumping to the nearest checkpoint and scanning forward. |
| **Virtualized table** | Only the rows currently visible in the viewport (~50) are ever drawn. |
| **Integer virtual scroll** | Scrolling is tracked by row index, not pixels, so it stays precise across billions of rows. |
| **Lazy parsing + cache** | CSV/JSON parsing runs only on visible rows. Work is bounded by viewport size, not file size. |
| **Background indexing** | Indexing runs on a worker thread with a live progress bar, so the UI stays responsive. |

## Supported formats

- **CSV** (`.csv`) — comma-separated. Columns are shown as `Column 1`, `Column 2`, …
- **TSV** (`.tsv`) — tab-separated. Columns are shown as `Column 1`, `Column 2`, …
- **JSON lines** (`.json`, `.ndjson`, `.jsonl`) — one JSON object per line (NDJSON). Object keys become columns.
- **Text** (`.txt`, `.log`, anything else) — one line per row.

> Every physical line is treated as data — Dinosaur does not hide the first line
> as a header. A single giant JSON array/object (not line-delimited) is
> intentionally not supported in streaming mode; use NDJSON for large JSON data.

## Usage

- **Open** — choose a file via the native dialog, drag & drop, or pass it as a CLI argument.
- **Go to** — type a row number and press <kbd>Enter</kbd> to jump.
- **Find** — search text; use **Previous**/**Next** and **Match case**.
- **Select & copy** — click or drag across cells, then right-click → **Copy** or press <kbd>Ctrl</kbd>+<kbd>C</kbd>.
- **Status bar** — shows format, row count, column count, and file size.

## Build from source

Requires the Rust toolchain (the repo pins `stable-x86_64-pc-windows-gnu`).

```powershell
cd app
# Release build is strongly recommended for large files
cargo run --release
```

## Project structure

```
app/
  src/
    main.rs     GUI (eframe/egui), state machine, virtualized table, search, selection
    index.rs    Sparse byte-offset line index (mmap + memchr + rayon)
    format.rs   Format detection, header generation, per-line parsing
  assets/       App icon (logo.svg)
  vendor/mesa/  Bundled CPU OpenGL renderer (shipped next to the exe)
  build.rs      Copies the Mesa DLLs next to the build output
  Cargo.toml    Dependencies and release profile (LTO enabled)
scripts/
  package.ps1   Builds + bundles the portable ZIP
  gen_csv.py    Generates large sample CSV files for testing
docs/           Product website (GitHub Pages)
.github/        Release workflow (builds the portable ZIP on each release)
```

## Releases

Releases are automated. Creating a GitHub Release (tag `vX.Y.Z`) triggers the
[workflow](.github/workflows/release.yml), which stamps the tag version into the
build, packages the portable ZIP, and attaches it to the release. The release
tag is the single source of truth for the version shown in the app.

## How it works

1. Pick a file → it is memory-mapped.
2. A worker thread scans newlines in parallel and builds the sparse index (progress bar shown).
3. Column headers are generated (`Column N` for CSV/TSV, object keys for NDJSON).
4. The virtualized table renders; as you scroll, only visible rows are located via the index and parsed.

## Tuning

`DEFAULT_SAMPLE` in [app/src/index.rs](app/src/index.rs) controls index density
(lines per checkpoint). Larger = less RAM but slightly slower per-line lookups;
smaller = more RAM, faster lookups. The default of 4096 is a good balance.

## License

MIT.

---

<div align="center">

Made with ❤️ in 🇮🇳 by [**Bipul Raman**](https://bipul.in)

</div>
