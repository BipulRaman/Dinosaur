//! Dinosaur — a native viewer for very large CSV / TSV / JSON / TXT
//! files. The whole file is memory-mapped and only the rows currently visible
//! in the viewport are parsed, so multi-gigabyte files open and scroll
//! smoothly.

// Hide the console window on Windows release builds (GUI app).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod format;
mod index;

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use memchr::{memchr, memrchr};
use memmap2::Mmap;

use format::Format;
use index::{LineIndex, DEFAULT_SAMPLE};

/// The app logo, authored as a vector file and embedded at compile time.
const LOGO_SVG: &[u8] = include_bytes!("../assets/logo.svg");

fn main() -> eframe::Result<()> {
    // Force Mesa's pure-CPU rasterizer (llvmpipe). Combined with the Mesa
    // `opengl32.dll` shipped next to the executable, this makes the app render
    // correctly even on machines without a usable GPU driver (e.g. cloud VMs).
    #[cfg(windows)]
    std::env::set_var("GALLIUM_DRIVER", "llvmpipe");

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([640.0, 400.0])
        .with_title("Dinosaur");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Dinosaur",
        options,
        Box::new(|cc| {
            // Pin a light theme so egui never switches with the system and
            // never restores the default blue selection highlight.
            cc.egui_ctx.options_mut(|o| {
                o.theme_preference = egui::ThemePreference::Light;
                o.fallback_theme = egui::Theme::Light;
            });
            egui_extras::install_image_loaders(&cc.egui_ctx);
            install_fonts(&cc.egui_ctx);
            setup_style(&cc.egui_ctx);
            let mut app = App::default();
            // Optionally open a file passed on the command line:
            //   Dinosaur.exe path\to\file.csv
            if let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) {
                if path.is_file() {
                    app.start_loading(path);
                }
            }
            Ok(Box::new(app))
        }),
    )
}

/// Rasterize the embedded SVG logo into an RGBA window/taskbar icon.
fn load_icon() -> Option<egui::IconData> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(LOGO_SVG, &opt).ok()?;
    let size = 256u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(egui::IconData {
        rgba: pixmap.data().to_vec(),
        width: size,
        height: size,
    })
}

/// Load a crisp native sans-serif (Segoe UI on Windows) as the proportional
/// font so the data grid reads like a modern spreadsheet. Falls back silently
/// to egui's bundled font if the system font isn't available.
fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 2] = [
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/arial.ttf",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("system-sans".to_owned(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "system-sans".to_owned());
            ctx.set_fonts(fonts);
            break;
        }
    }
}

/// Apply a refined dark theme: tuned typography, spacing, rounded widgets and
/// neutral Apple-style grey controls, so the app feels like a finished product.
fn setup_style(ctx: &egui::Context) {
    use egui::{Color32, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle, Vec2};

    let mut style = (*ctx.style()).clone();

    // Typography.
    style.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
    ]
    .into();

    // Spacing.
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.window_margin = Margin::same(10.0);
    style.spacing.menu_margin = Margin::same(8.0);

    // Always show solid (non-floating) scrollbars so they never disappear or
    // overlap content. `solid()` also sets `foreground_color = false`, so the
    // handle is painted with the grey `bg_fill` instead of the near-black
    // text colour used by the default floating style.
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.spacing.scroll.bar_width = 12.0;

    // Visuals — refined light theme with neutral Apple-style grey controls.
    let mut v = egui::Visuals::light();
    let rounding = Rounding::same(3.0);

    v.panel_fill = Color32::from_rgb(245, 245, 247);
    v.window_fill = Color32::from_rgb(255, 255, 255);
    v.extreme_bg_color = Color32::from_rgb(255, 255, 255);
    v.faint_bg_color = Color32::from_rgb(244, 244, 246);
    v.window_rounding = Rounding::same(4.0);
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(214, 214, 218));

    // Base text colour for plain (noninteractive) labels. `weak_text_color()`
    // — used by `.weak()` labels, the status bar and TextEdit hint text — is a
    // 50% blend between this colour and `window_fill` (white), so a darker base
    // here yields noticeably less-washed-out faded text.
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(24, 24, 28));

    v.selection.bg_fill = Color32::from_rgb(214, 214, 220);
    v.selection.stroke = Stroke::new(1.0, Color32::BLACK);
    v.hyperlink_color = Color32::from_rgb(96, 96, 102);

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = rounding;
    }

    // Neutral, Apple-style grey controls. `weak_bg_fill` paints button
    // backgrounds; `bg_fill` paints the scrollbar handle — keep the latter a
    // clearly visible medium grey (never black).
    v.widgets.inactive.bg_fill = Color32::from_rgb(193, 193, 198);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(236, 236, 238);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(208, 208, 212));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(40, 40, 44));

    v.widgets.hovered.bg_fill = Color32::from_rgb(168, 168, 174);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(224, 224, 228);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(188, 188, 194));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::BLACK);

    v.widgets.active.bg_fill = Color32::from_rgb(148, 148, 154);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(208, 208, 214);
    v.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(168, 168, 174));

    style.visuals = v;
    ctx.set_style(style);
}

/// Successfully loaded file plus its index and parsed header.
struct Loaded {
    path: PathBuf,
    mmap: Arc<Mmap>,
    index: LineIndex,
    format: Format,
    headers: Vec<String>,
    /// First physical line number that contains data (1 if there is a header).
    first_data_line: u64,
    /// Cache of parsed rows keyed by data-row number.
    cache: HashMap<u64, Arc<Vec<String>>>,
    /// Index of the first data row currently displayed at the top of the
    /// viewport. This drives a precision-safe, integer-based virtual scroll:
    /// egui/egui_extras compute scroll offsets in `f32`, which loses precision
    /// past ~16M pixels, so with tens of millions of rows their pixel-based
    /// virtualization misplaces rows (gap at top, hidden last rows). We scroll
    /// by row index instead and only ever render the visible window.
    top_row: u64,
    /// Sub-row scroll remainder (pixels) accumulated from the mouse wheel so
    /// that slow scrolling still advances smoothly one row at a time.
    scroll_residual: f32,
    /// Current cell-range selection (anchor + focus), if any.
    sel: Option<CellSel>,
    /// True while the primary mouse button is held during a drag-select.
    selecting: bool,
}

/// A rectangular cell-range selection, like a spreadsheet. `anchor` is where
/// the drag started; `focus` is the cell currently under the pointer.
#[derive(Clone, Copy)]
struct CellSel {
    anchor: (u64, usize),
    focus: (u64, usize),
}

impl CellSel {
    fn single(row: u64, col: usize) -> Self {
        CellSel {
            anchor: (row, col),
            focus: (row, col),
        }
    }
    fn row_range(&self) -> std::ops::RangeInclusive<u64> {
        self.anchor.0.min(self.focus.0)..=self.anchor.0.max(self.focus.0)
    }
    fn col_range(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor.1.min(self.focus.1)..=self.anchor.1.max(self.focus.1)
    }
    fn contains(&self, row: u64, col: usize) -> bool {
        self.row_range().contains(&row) && self.col_range().contains(&col)
    }
}

impl Loaded {
    fn data_rows(&self) -> u64 {
        self.index.total_lines.saturating_sub(self.first_data_line)
    }

    /// Fetch (and cache) the parsed cells for data row `row`.
    fn row(&mut self, row: u64) -> Arc<Vec<String>> {
        if let Some(hit) = self.cache.get(&row) {
            return hit.clone();
        }
        if self.cache.len() > 4096 {
            self.cache.clear();
        }
        let physical = row + self.first_data_line;
        let bytes = self.index.line_bytes(&self.mmap, physical);
        let cells = Arc::new(format::parse_row(self.format, &self.headers, bytes));
        self.cache.insert(row, cells.clone());
        cells
    }
}

/// Message sent from the background indexing thread to the UI.
enum LoadMsg {
    Done(Box<Loaded>),
    Error(String),
}

enum State {
    Idle,
    Loading {
        path: PathBuf,
        progress: Arc<AtomicU64>,
        total: u64,
        started: Instant,
        rx: Receiver<LoadMsg>,
    },
    Loaded(Box<Loaded>),
    Error(String),
}

struct App {
    state: State,
    goto: String,
    scroll_to_row: Option<usize>,
    /// Current search query.
    search: String,
    /// Whether matching is case-sensitive (default: case-insensitive).
    search_case_sensitive: bool,
    /// Data row of the most recent match (used as the anchor for Next/Prev).
    last_match: Option<u64>,
    /// Short status message shown next to the search box.
    search_status: String,
}

impl Default for App {
    fn default() -> Self {
        App {
            state: State::Idle,
            goto: String::new(),
            scroll_to_row: None,
            search: String::new(),
            search_case_sensitive: false,
            last_match: None,
            search_status: String::new(),
        }
    }
}

impl App {
    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Data files", &["csv", "tsv", "json", "ndjson", "jsonl", "txt", "log"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.start_loading(path);
        }
    }

    fn start_loading(&mut self, path: PathBuf) {
        let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let progress = Arc::new(AtomicU64::new(0));
        let (tx, rx): (Sender<LoadMsg>, Receiver<LoadMsg>) = std::sync::mpsc::channel();
        let progress_thread = progress.clone();
        let path_thread = path.clone();

        std::thread::spawn(move || {
            let msg = load_file(path_thread, &progress_thread);
            let _ = tx.send(msg);
        });

        // Reset any previous search when a new file starts loading.
        self.last_match = None;
        self.search_status.clear();

        self.state = State::Loading {
            path,
            progress,
            total,
            started: Instant::now(),
            rx,
        };
    }

    /// Run a search in the given direction, updating the scroll target and the
    /// status message. Matching scans the memory-mapped bytes directly, so it
    /// is fast even on multi-gigabyte files.
    fn find(&mut self, dir: Direction) {
        let query = self.search.clone();
        if query.is_empty() {
            self.search_status.clear();
            return;
        }
        let case_insensitive = !self.search_case_sensitive;
        let anchor = self.last_match;
        let result = if let State::Loaded(loaded) = &self.state {
            search_rows(loaded, query.as_bytes(), anchor, dir, case_insensitive)
        } else {
            return;
        };
        match result {
            Some((row, wrapped)) => {
                self.last_match = Some(row);
                self.scroll_to_row = Some(row as usize);
                self.search_status = format!(
                    "Row {}{}",
                    row + 1,
                    if wrapped { " (wrapped)" } else { "" }
                );
            }
            None => {
                self.search_status = "No match".to_string();
            }
        }
    }
}

/// Search direction for [`App::find`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Backward,
}

/// Open + memory-map the file and build its line index. Runs on a worker thread.
fn load_file(path: PathBuf, progress: &AtomicU64) -> LoadMsg {
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => return LoadMsg::Error(format!("Cannot open file: {e}")),
    };
    let mmap = match unsafe { Mmap::map(&file) } {
        Ok(m) => Arc::new(m),
        Err(e) => return LoadMsg::Error(format!("Cannot memory-map file: {e}")),
    };

    let index = LineIndex::build(&mmap, DEFAULT_SAMPLE, progress);
    let fmt = Format::from_path(&path);

    let first_line = if index.total_lines > 0 {
        index.line_bytes(&mmap, 0).to_vec()
    } else {
        Vec::new()
    };
    let headers = format::headers(fmt, &first_line);
    let first_data_line = if fmt.has_header() && index.total_lines > 0 {
        1
    } else {
        0
    };

    LoadMsg::Done(Box::new(Loaded {
        path,
        mmap,
        index,
        format: fmt,
        headers,
        first_data_line,
        cache: HashMap::new(),
        top_row: 0,
        scroll_residual: 0.0,
        sel: None,
        selecting: false,
    }))
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

/// Locate the next/previous data row matching `needle`, starting relative to
/// `anchor` (the previously matched row). Returns `(row, wrapped)` where
/// `wrapped` indicates the search wrapped around the end/start of the file.
fn search_rows(
    loaded: &Loaded,
    needle: &[u8],
    anchor: Option<u64>,
    dir: Direction,
    ci: bool,
) -> Option<(u64, bool)> {
    let total = loaded.data_rows();
    if total == 0 || needle.is_empty() {
        return None;
    }
    let data: &[u8] = &loaded.mmap;
    let idx = &loaded.index;
    let fdl = loaded.first_data_line;
    let row_to_offset = |r: u64| idx.line_start(data, r + fdl);
    let off_to_row = |o: usize| idx.line_at_offset(data, o).saturating_sub(fdl);

    match dir {
        Direction::Forward => {
            let start_row = anchor.map(|a| a + 1).unwrap_or(0);
            if start_row < total {
                let from = row_to_offset(start_row);
                if let Some(p) = find_fwd(data, needle, from, ci) {
                    return Some((off_to_row(p), false));
                }
            }
            // Wrap around to the top.
            if let Some(p) = find_fwd(data, needle, row_to_offset(0), ci) {
                let row = off_to_row(p);
                let wrapped = anchor.map_or(false, |a| row <= a);
                return Some((row, wrapped));
            }
            None
        }
        Direction::Backward => {
            let end = match anchor {
                Some(a) => row_to_offset(a),
                None => data.len(),
            };
            if let Some(p) = find_bwd(data, needle, end, ci) {
                return Some((off_to_row(p), false));
            }
            // Wrap around to the bottom.
            if let Some(p) = find_bwd(data, needle, data.len(), ci) {
                let row = off_to_row(p);
                let wrapped = anchor.map_or(false, |a| row >= a);
                return Some((row, wrapped));
            }
            None
        }
    }
}

/// Case-(in)sensitive equality of two equal-length byte slices.
fn eq_at(a: &[u8], b: &[u8], ci: bool) -> bool {
    if ci {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// First index in `slice` whose byte equals `b` (either case if `ci`).
fn first_pos(slice: &[u8], b: u8, ci: bool) -> Option<usize> {
    let lo = b.to_ascii_lowercase();
    let up = b.to_ascii_uppercase();
    if !ci || lo == up {
        memchr(b, slice)
    } else {
        match (memchr(lo, slice), memchr(up, slice)) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (Some(x), None) => Some(x),
            (None, Some(y)) => Some(y),
            (None, None) => None,
        }
    }
}

/// Last index in `slice` whose byte equals `b` (either case if `ci`).
fn last_pos(slice: &[u8], b: u8, ci: bool) -> Option<usize> {
    let lo = b.to_ascii_lowercase();
    let up = b.to_ascii_uppercase();
    if !ci || lo == up {
        memrchr(b, slice)
    } else {
        match (memrchr(lo, slice), memrchr(up, slice)) {
            (Some(x), Some(y)) => Some(x.max(y)),
            (Some(x), None) => Some(x),
            (None, Some(y)) => Some(y),
            (None, None) => None,
        }
    }
}

/// Forward substring search in `hay` starting at byte `from`.
fn find_fwd(hay: &[u8], needle: &[u8], from: usize, ci: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let last_start = hay.len() - needle.len();
    let first = needle[0];
    let mut i = from.min(last_start + 1);
    while i <= last_start {
        let rel = first_pos(&hay[i..=last_start], first, ci)?;
        let p = i + rel;
        if eq_at(&hay[p..p + needle.len()], needle, ci) {
            return Some(p);
        }
        i = p + 1;
    }
    None
}

/// Backward substring search: last match whose end is `<= end`.
fn find_bwd(hay: &[u8], needle: &[u8], end: usize, ci: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > end {
        return None;
    }
    let first = needle[0];
    let mut hi = end - needle.len(); // last valid start index (inclusive)
    loop {
        let rel = last_pos(&hay[..=hi], first, ci)?;
        let p = rel;
        if eq_at(&hay[p..p + needle.len()], needle, ci) {
            return Some(p);
        }
        if p == 0 {
            return None;
        }
        hi = p - 1;
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Transition out of the loading state when the worker finishes.
        if let State::Loading { rx, .. } = &self.state {
            if let Ok(msg) = rx.try_recv() {
                match msg {
                    LoadMsg::Done(loaded) => self.state = State::Loaded(loaded),
                    LoadMsg::Error(e) => self.state = State::Error(e),
                }
            } else {
                ctx.request_repaint(); // keep the progress bar animating
            }
        }

        // Open a file that was dragged onto the window.
        if let Some(path) = ctx
            .input(|i| i.raw.dropped_files.clone())
            .into_iter()
            .find_map(|f| f.path)
        {
            self.start_loading(path);
        }

        self.top_bar(ctx);
        self.status_bar(ctx);

        let mut open_clicked = false;
        egui::CentralPanel::default().show(ctx, |ui| match &mut self.state {
            State::Idle => {
                ui.vertical_centered(|ui| {
                    ui.add_space(72.0);
                    ui.add(
                        egui::Image::new(egui::ImageSource::Bytes {
                            uri: "bytes://logo.svg".into(),
                            bytes: LOGO_SVG.into(),
                        })
                        .fit_to_exact_size(egui::vec2(96.0, 96.0)),
                    );
                    ui.add_space(16.0);
                    ui.heading("Dinosaur");
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Open very large CSV, TSV, JSON, or TXT files instantly.",
                        )
                        .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(22.0);
                    let btn = egui::Button::new(egui::RichText::new("Open file…").size(15.0))
                        .min_size(egui::vec2(170.0, 40.0));
                    if ui.add(btn).clicked() {
                        open_clicked = true;
                    }
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("…or drag and drop a file anywhere")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            }
            State::Loading {
                path,
                progress,
                total,
                started,
                ..
            } => {
                let done = progress.load(Ordering::Relaxed);
                let frac = if *total > 0 {
                    (done as f32 / *total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ui.vertical_centered(|ui| {
                    ui.add_space(96.0);
                    ui.add(egui::Spinner::new().size(36.0));
                    ui.add_space(14.0);
                    ui.heading("Indexing…");
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        )
                        .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(14.0);
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .show_percentage()
                            .desired_width(420.0)
                            .fill(egui::Color32::from_rgb(150, 150, 156)),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} / {} · {:.1}s",
                            human_bytes(done),
                            human_bytes(*total),
                            started.elapsed().as_secs_f32()
                        ))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                });
            }
            State::Error(e) => {
                ui.vertical_centered(|ui| {
                    ui.add_space(96.0);
                    ui.label(
                        egui::RichText::new("!")
                            .size(40.0)
                            .strong()
                            .color(egui::Color32::from_rgb(239, 83, 80)),
                    );
                    ui.add_space(8.0);
                    ui.heading("Couldn't open file");
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(e.as_str()).color(ui.visuals().weak_text_color()));
                });
            }
            State::Loaded(loaded) => {
                let scroll_to = self.scroll_to_row.take();
                let highlight = self.last_match;
                show_table(ui, loaded, scroll_to, highlight);
            }
        });

        if open_clicked {
            self.open_dialog();
        }

        // Visual feedback while a file is hovered over the window.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(170));
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Drop file to open",
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }
    }
}

impl App {
    fn top_bar(&mut self, ctx: &egui::Context) {
        let stroke = egui::Stroke::new(1.0, ctx.style().visuals.window_stroke.color);
        egui::TopBottomPanel::top("top")
            .exact_height(54.0)
            .frame(
                egui::Frame::none()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(16.0, 0.0))
                    .stroke(egui::Stroke::NONE),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                // Bottom hairline divider for a crisp, app-like edge.
                let r = ui.max_rect();
                ui.painter().hline(
                    r.x_range(),
                    r.bottom() - 0.5,
                    stroke,
                );

                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;

                    if ui
                        .add_sized([72.0, 32.0], egui::Button::new("Open"))
                        .on_hover_text("Open a file (drag & drop also works)")
                        .clicked()
                    {
                        self.open_dialog();
                    }

                    if let State::Loaded(loaded) = &self.state {
                        let rows = loaded.data_rows();
                        toolbar_divider(ui);

                        ui.label(egui::RichText::new("Go to").weak());
                        let resp = ui.add_sized(
                            [88.0, 30.0],
                            egui::TextEdit::singleline(&mut self.goto)
                                .hint_text("row #")
                                .vertical_align(egui::Align::Center),
                        );
                        let go = ui.add_sized([44.0, 30.0], egui::Button::new("Go")).clicked()
                            || (resp.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        if go {
                            if let Ok(n) = self.goto.trim().parse::<u64>() {
                                let target = n.saturating_sub(1).min(rows.saturating_sub(1));
                                self.scroll_to_row = Some(target as usize);
                            }
                        }

                        toolbar_divider(ui);

                        ui.label(egui::RichText::new("Find").weak());
                        let sresp = ui.add_sized(
                            [220.0, 30.0],
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("Find text…")
                                .vertical_align(egui::Align::Center),
                        );
                        if sresp.changed() {
                            // Editing the query restarts the search from the top.
                            self.last_match = None;
                            self.search_status.clear();
                        }
                        let enter =
                            sresp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let prev = ui
                            .add_sized([56.0, 30.0], egui::Button::new("Previous"))
                            .on_hover_text("Previous match")
                            .clicked();
                        let next = ui
                            .add_sized([56.0, 30.0], egui::Button::new("Next"))
                            .on_hover_text("Next match (Enter)")
                            .clicked()
                            || enter;
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.add_space(2.0);
                        let mut case = self.search_case_sensitive;
                        if ui
                            .checkbox(&mut case, "Match case")
                            .on_hover_text("Match case")
                            .changed()
                        {
                            self.search_case_sensitive = case;
                            self.last_match = None;
                            self.search_status.clear();
                        }
                        if next {
                            self.find(Direction::Forward);
                        } else if prev {
                            self.find(Direction::Backward);
                        }
                        if !self.search_status.is_empty() {
                            ui.add_space(4.0);
                            let color = if self.last_match.is_some() {
                                egui::Color32::from_rgb(52, 168, 83)
                            } else {
                                egui::Color32::from_rgb(217, 64, 56)
                            };
                            ui.label(egui::RichText::new(&self.search_status).color(color).small());
                        }
                    }
                });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::none()
                    .fill(ctx.style().visuals.extreme_bg_color)
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0)),
            )
            .show(ctx, |ui| {
                let weak = ui.visuals().weak_text_color();
                ui.horizontal(|ui| match &self.state {
                    State::Loaded(loaded) => {
                        ui.label(
                            egui::RichText::new(
                                loaded
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            )
                            .strong(),
                        );

                        // Right-aligned metadata.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(human_bytes(loaded.index.file_len))
                                    .color(weak),
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!("{} cols", loaded.headers.len()))
                                    .color(weak),
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} rows",
                                    format_count(loaded.data_rows())
                                ))
                                .color(weak),
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(loaded.format.label())
                                    .strong(),
                            );
                        });
                    }
                    State::Idle => {
                        ui.label(egui::RichText::new("Ready").color(weak));
                    }
                    State::Loading { .. } => {
                        ui.label(egui::RichText::new("Indexing…").color(weak));
                    }
                    State::Error(_) => {
                        ui.label(
                            egui::RichText::new("Error")
                                .color(egui::Color32::from_rgb(239, 83, 80)),
                        );
                    }
                });
            });
    }
}

/// Spreadsheet palette (Google Sheets-like).
const GRID_LINE: egui::Color32 = egui::Color32::from_rgb(218, 220, 224); // #dadce0
const HEADER_BG: egui::Color32 = egui::Color32::from_rgb(248, 249, 250); // #f8f9fa
const HEADER_TEXT: egui::Color32 = egui::Color32::from_rgb(95, 99, 104); // #5f6368
const CELL_BG: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const CELL_TEXT: egui::Color32 = egui::Color32::from_rgb(32, 33, 36); // #202124
const NUM_TEXT: egui::Color32 = egui::Color32::from_rgb(128, 132, 138);
const SEL_BG: egui::Color32 = egui::Color32::from_rgb(232, 240, 254); // #e8f0fe
const SEL_HEADER: egui::Color32 = egui::Color32::from_rgb(174, 203, 250); // #aecbfa
const SEL_BORDER: egui::Color32 = egui::Color32::from_rgb(26, 115, 232); // #1a73e8
const FIND_BG: egui::Color32 = egui::Color32::from_rgb(255, 241, 194); // soft yellow

/// Fill a table cell and draw its gridlines, returning the cell rectangle.
///
/// Every cell paints its right + bottom border; the left/top borders are only
/// drawn for the first column / header row so the outer edges of the grid are
/// closed. Because each cell paints relative to its own `max_rect`, the lines
/// track the columns even as they are resized.
fn paint_cell(ui: &egui::Ui, fill: Option<egui::Color32>, left: bool, top: bool) -> egui::Rect {
    let rect = ui.max_rect();
    let p = ui.painter();
    if let Some(f) = fill {
        p.rect_filled(rect, 0.0, f);
    }
    let stroke = egui::Stroke::new(1.0, GRID_LINE);
    p.hline(rect.x_range(), rect.bottom() - 0.5, stroke);
    p.vline(rect.right() - 0.5, rect.y_range(), stroke);
    if left {
        p.vline(rect.left() + 0.5, rect.y_range(), stroke);
    }
    if top {
        p.hline(rect.x_range(), rect.top() + 0.5, stroke);
    }
    rect
}

/// Render the virtualized data table. Only visible rows are parsed/drawn.
///
/// We do **not** use egui_extras' built-in vertical scroll for paging through
/// rows. egui and egui_extras track the scroll offset and the virtual content
/// height as `f32`, which only has ~24 bits of mantissa (≈16.7M integer
/// precision). With tens of millions of rows the content is ~1e9 px tall, so
/// the offset snaps to ~64–128 px steps and the row buffers no longer line up
/// with the scroll position — the symptom being a gap at the top and hidden
/// last rows that gets worse toward the end of the file.
///
/// Instead we scroll by *row index* (`u64`) and render only the visible window,
/// driving it with a custom scrollbar that maps position with `f64`/integer
/// math. egui_extras still lays out the visible window (columns, striping,
/// resizing); its own vertical scroll is disabled.
fn show_table(
    ui: &mut egui::Ui,
    loaded: &mut Loaded,
    scroll_to: Option<usize>,
    highlight: Option<u64>,
) {
    let total_rows = loaded.data_rows();
    if total_rows == 0 {
        return;
    }
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 9.0;
    let spacing_y = 0.0;
    let row_pitch = row_height + spacing_y;
    let ncols = loaded.headers.len().max(1);

    // Size the row-number column to the widest number it must show.
    let digits = (total_rows.max(1) as f64).log10().floor() as f32 + 1.0;
    let char_w = ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), '0'));
    let num_w = (digits * char_w + 24.0).max(56.0);
    const COL_W: f32 = 180.0;
    let content_w = num_w + ncols as f32 * COL_W;

    let header_h = row_height + 4.0;
    let scrollbar_h = ui.spacing().scroll.bar_width
        + ui.spacing().scroll.bar_inner_margin
        + ui.spacing().scroll.bar_outer_margin;

    const SB_W: f32 = 12.0;
    const SB_GAP: f32 = 2.0;

    let outer = ui.available_rect_before_wrap();

    // Work out how many rows fit, and whether the vertical / horizontal
    // scrollbars are needed. There is a mild circular dependency (the vertical
    // scrollbar steals width, the horizontal scrollbar steals height), so we
    // estimate once and then settle.
    let body_h_full = (outer.height() - header_h - spacing_y).max(0.0);
    let visible_full_est = (body_h_full / row_pitch).floor().max(1.0) as u64;
    let vneed = total_rows > visible_full_est;
    let table_w_avail = outer.width() - if vneed { SB_W + SB_GAP } else { 0.0 };
    let needs_hscroll = content_w > table_w_avail;
    let body_h = (body_h_full - if needs_hscroll { scrollbar_h } else { 0.0 }).max(0.0);
    let visible_full = (body_h / row_pitch).floor().max(1.0) as u64;
    // Draw a couple of extra rows so the partially-visible bottom row is filled;
    // anything past the body is clipped by the horizontal scroll area.
    let visible_draw = (visible_full + 2).min(total_rows);
    let max_top = total_rows.saturating_sub(visible_full);

    let table_w = content_w.max(table_w_avail);
    let table_rect = egui::Rect::from_min_max(
        outer.min,
        egui::pos2(
            outer.right() - if vneed { SB_W + SB_GAP } else { 0.0 },
            outer.bottom(),
        ),
    );

    // --- Vertical navigation input (wheel, keys) ------------------------------
    let pointer_over = ui
        .input(|i| i.pointer.hover_pos())
        .map_or(false, |p| outer.contains(p));
    let typing = ui.memory(|m| m.focused().is_some());
    let (wheel_dy, pg_dn, pg_up, key_home, key_end, arr_dn, arr_up) = ui.input(|i| {
        (
            i.smooth_scroll_delta.y,
            i.key_pressed(egui::Key::PageDown),
            i.key_pressed(egui::Key::PageUp),
            i.key_pressed(egui::Key::Home),
            i.key_pressed(egui::Key::End),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::ArrowUp),
        )
    });

    let mut top = loaded.top_row as i64;
    if pointer_over && wheel_dy != 0.0 {
        loaded.scroll_residual += wheel_dy;
        let steps = (loaded.scroll_residual / row_pitch).trunc();
        loaded.scroll_residual -= steps * row_pitch;
        top -= steps as i64; // positive wheel delta scrolls the view up
    }
    if !typing {
        if pg_dn {
            top += visible_full as i64;
        }
        if pg_up {
            top -= visible_full as i64;
        }
        if arr_dn {
            top += 1;
        }
        if arr_up {
            top -= 1;
        }
        if key_home {
            top = 0;
        }
        if key_end {
            top = max_top as i64;
        }
    }
    // A "Go to row" / search jump always wins and is shown at the top.
    if let Some(r) = scroll_to {
        top = (r as i64).min(max_top as i64);
        loaded.scroll_residual = 0.0;
    }
    loaded.top_row = top.clamp(0, max_top as i64) as u64;

    // --- Custom vertical scrollbar -------------------------------------------
    if vneed && max_top > 0 {
        let track_top = outer.top() + header_h + spacing_y;
        let track_rect = egui::Rect::from_min_max(
            egui::pos2(outer.right() - SB_W, track_top),
            egui::pos2(outer.right(), track_top + body_h),
        );
        let resp = ui.interact(
            track_rect,
            ui.id().with("vscrollbar"),
            egui::Sense::click_and_drag(),
        );

        let handle_frac = (visible_full as f32 / total_rows as f32).clamp(0.04, 1.0);
        let handle_h = (body_h * handle_frac).clamp(28.0, body_h);
        let travel = (body_h - handle_h).max(0.0);

        if (resp.dragged() || resp.clicked()) && travel > 0.0 {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = ((p.y - track_top - handle_h * 0.5) / travel).clamp(0.0, 1.0);
                loaded.top_row = (t as f64 * max_top as f64).round() as u64;
            }
        }

        let pos_frac = if max_top > 0 {
            (loaded.top_row as f64 / max_top as f64) as f32
        } else {
            0.0
        };
        let handle_top = track_top + travel * pos_frac;
        let handle_rect = egui::Rect::from_min_size(
            egui::pos2(outer.right() - SB_W + 1.0, handle_top),
            egui::vec2(SB_W - 2.0, handle_h),
        );
        let v = ui.visuals();
        let handle_color = if resp.dragged() {
            v.widgets.active.bg_fill
        } else if resp.hovered() {
            v.widgets.hovered.bg_fill
        } else {
            v.widgets.inactive.bg_fill
        };
        ui.painter().rect_filled(
            handle_rect,
            egui::Rounding::same((SB_W - 2.0) * 0.5),
            handle_color,
        );
    }

    // --- Render the visible window -------------------------------------------
    let start = loaded.top_row;
    let count = visible_draw.min(total_rows - start) as usize;

    // Snapshot the selection for read-only fills; live edits below take effect
    // on the next repaint (which we request while dragging).
    let sel_view = loaded.sel;
    // Set by the context-menu "Copy"; consumed after the table renders so the
    // copy code can borrow `loaded` without fighting the row closures.
    let copy_request = std::cell::Cell::new(false);
    // Screen-space bounds of the visible selection, used to stroke its border.
    let sel_bounds: std::cell::Cell<Option<egui::Rect>> = std::cell::Cell::new(None);

    let primary_down = ui.input(|i| i.pointer.primary_down());
    let shift = ui.input(|i| i.modifiers.shift);
    // Pointer position in screen space. We hit-test cells against this directly
    // rather than relying on per-widget `hovered()`, because egui locks pointer
    // interaction to the origin widget for the duration of a drag.
    let pointer_pos = ui.input(|i| i.pointer.interact_pos());

    ui.scope_builder(egui::UiBuilder::new().max_rect(table_rect), |ui| {
        // egui_extras' Table only scrolls vertically; wrap it in a horizontal
        // scroll area for wide files. The table's own vertical scroll is
        // disabled — we page through rows ourselves — so only the small visible
        // window is ever laid out and there is no f32 precision problem.
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .drag_to_scroll(false)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_width(table_w);

                let mut builder = TableBuilder::new(ui)
                    .striped(false)
                    .resizable(true)
                    .vscroll(false)
                    .auto_shrink([false, false])
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(num_w));
                for i in 0..ncols {
                    let col = if i + 1 == ncols {
                        Column::remainder().at_least(COL_W).clip(true)
                    } else {
                        Column::initial(COL_W).at_least(40.0).clip(true)
                    };
                    builder = builder.column(col);
                }

                builder
                    .header(header_h, |mut header| {
                        let corner_sel = sel_view.is_some();
                        header.col(|ui| {
                            paint_cell(
                                ui,
                                Some(if corner_sel { SEL_HEADER } else { HEADER_BG }),
                                true,
                                true,
                            );
                        });
                        for (i, name) in loaded.headers.iter().enumerate() {
                            let col_sel =
                                sel_view.map_or(false, |s| s.col_range().contains(&i));
                            header.col(|ui| {
                                paint_cell(
                                    ui,
                                    Some(if col_sel { SEL_HEADER } else { HEADER_BG }),
                                    false,
                                    true,
                                );
                                ui.add_space(8.0);
                                let mut t = egui::RichText::new(name)
                                    .color(if col_sel { SEL_BORDER } else { HEADER_TEXT });
                                if col_sel {
                                    t = t.strong();
                                }
                                ui.add(egui::Label::new(t).selectable(false).truncate());
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_height, count, |mut row| {
                            let abs = start + row.index() as u64;
                            let row_in_sel =
                                sel_view.map_or(false, |s| s.row_range().contains(&abs));
                            let found = highlight == Some(abs);

                            // Row-number cell.
                            row.col(|ui| {
                                let fill = if row_in_sel { SEL_HEADER } else { HEADER_BG };
                                paint_cell(ui, Some(fill), true, false);
                                ui.centered_and_justified(|ui| {
                                    let mut t = egui::RichText::new((abs + 1).to_string())
                                        .color(if row_in_sel { SEL_BORDER } else { NUM_TEXT });
                                    if row_in_sel {
                                        t = t.strong();
                                    }
                                    ui.add(egui::Label::new(t).selectable(false));
                                });
                            });

                            let cells = loaded.row(abs);
                            for c in 0..ncols {
                                let in_sel = sel_view.map_or(false, |s| s.contains(abs, c));
                                row.col(|ui| {
                                    let fill = if in_sel {
                                        SEL_BG
                                    } else if found {
                                        FIND_BG
                                    } else {
                                        CELL_BG
                                    };
                                    let rect = paint_cell(ui, Some(fill), false, false);
                                    if in_sel {
                                        let cur = sel_bounds.get();
                                        sel_bounds
                                            .set(Some(cur.map_or(rect, |b| b.union(rect))));
                                    }

                                    // One interactive surface per cell so a click
                                    // or right-click anywhere in the cell works.
                                    let resp = ui.interact(
                                        rect,
                                        egui::Id::new(("cell", abs, c)),
                                        egui::Sense::click_and_drag(),
                                    );

                                    // Start the selection (plain click or drag),
                                    // or extend it with Shift+click.
                                    if resp.drag_started() || (resp.clicked() && !shift) {
                                        loaded.sel = Some(CellSel::single(abs, c));
                                        loaded.selecting = true;
                                    } else if shift && resp.clicked() {
                                        match &mut loaded.sel {
                                            Some(s) => s.focus = (abs, c),
                                            None => loaded.sel = Some(CellSel::single(abs, c)),
                                        }
                                    }
                                    // Extend while dragging: hit-test the pointer
                                    // directly (see `pointer_pos` note above).
                                    if loaded.selecting && primary_down {
                                        if let Some(p) = pointer_pos {
                                            if rect.contains(p) {
                                                if let Some(s) = &mut loaded.sel {
                                                    s.focus = (abs, c);
                                                }
                                            }
                                        }
                                    }
                                    // Right-click selects the cell unless it is
                                    // already inside the current selection.
                                    if resp.secondary_clicked() {
                                        let inside = loaded
                                            .sel
                                            .map_or(false, |s| s.contains(abs, c));
                                        if !inside {
                                            loaded.sel = Some(CellSel::single(abs, c));
                                        }
                                    }

                                    ui.add_space(8.0);
                                    let text =
                                        cells.get(c).map(String::as_str).unwrap_or("");
                                    if !text.is_empty() {
                                        // `truncate()` shows the full value as a
                                        // tooltip when the text is clipped.
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(text).color(CELL_TEXT),
                                            )
                                            .selectable(false)
                                            .truncate(),
                                        );
                                    }

                                    resp.context_menu(|ui| {
                                        if ui.button("Copy").clicked() {
                                            copy_request.set(true);
                                            ui.close_menu();
                                        }
                                    });
                                });
                            }
                        });
                    });

                // Spreadsheet-style border around the selected range.
                if let Some(b) = sel_bounds.get() {
                    ui.painter()
                        .rect_stroke(b, 0.0, egui::Stroke::new(2.0, SEL_BORDER));
                }
            });
    });

    // Release the drag when the mouse button is up.
    if !primary_down {
        loaded.selecting = false;
    }
    if loaded.selecting {
        ui.ctx().request_repaint();
    }

    // Copy the selection — context-menu "Copy" or Ctrl/Cmd+C — as TSV.
    let ctrl_c =
        !typing && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::C));
    if copy_request.get() || ctrl_c {
        if let Some(text) = selection_to_tsv(loaded, ncols) {
            ui.ctx().copy_text(text);
        }
    }
}

/// Serialize the current cell selection as tab-separated rows (TSV), suitable
/// for pasting into a spreadsheet. Returns `None` if nothing is selected.
fn selection_to_tsv(loaded: &mut Loaded, ncols: usize) -> Option<String> {
    let sel = loaded.sel?;
    let cols = sel.col_range();
    let mut out = String::new();
    for r in sel.row_range() {
        let cells = loaded.row(r);
        let mut first = true;
        for c in cols.clone() {
            if c >= ncols {
                continue;
            }
            if !first {
                out.push('\t');
            }
            first = false;
            if let Some(v) = cells.get(c) {
                out.push_str(v);
            }
        }
        out.push('\n');
    }
    if out.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

/// Draw a short, subtle vertical divider between toolbar groups (lighter and
/// inset compared to the full-height `ui.separator()`).
fn toolbar_divider(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let h = 22.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, ui.available_height()), egui::Sense::hover());
    let cy = rect.center().y;
    let color = ui.visuals().window_stroke.color;
    ui.painter().vline(
        rect.center().x,
        (cy - h / 2.0)..=(cy + h / 2.0),
        egui::Stroke::new(1.0, color),
    );
    ui.add_space(4.0);
}

/// Format a row count with thousands separators (e.g. `1,234,567`).
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
