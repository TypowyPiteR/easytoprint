#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextStyle,
};

mod print;

/// Pause between jobs, so the printer spooler can keep up.
const PAUSE_BETWEEN_JOBS: Duration = Duration::from_millis(2000);
/// Printer preselected at startup, if installed on this machine.
const DEFAULT_PRINTER: &str = "Canon MF750C Series UFR II";

/// queue row height (fixed — rows never stretch)
const ROW_H: f32 = 34.0;
/// vertical gap between rows (the separator sits in its middle)
const ROW_GAP: f32 = 6.0;
/// distance between row tops
const STRIDE: f32 = ROW_H + ROW_GAP;
/// table header height
const HEAD_H: f32 = 24.0;
/// width of the status column (badge + remove button)
const RIGHT_ZONE: f32 = 150.0;

// ---------------------------------------------------------------------------
// Palette: light "paper & print shop" theme
// ---------------------------------------------------------------------------

mod theme {
    use eframe::egui::Color32;

    /// application background — warm paper white
    pub const PAPER: Color32 = Color32::from_rgb(247, 246, 243);
    /// cards — pure white
    pub const CARD: Color32 = Color32::from_rgb(255, 255, 255);
    /// primary text — ink
    pub const INK: Color32 = Color32::from_rgb(25, 28, 31);
    /// secondary text
    pub const INK_SOFT: Color32 = Color32::from_rgb(110, 116, 128);
    /// thin borders
    pub const HAIRLINE: Color32 = Color32::from_rgb(231, 229, 224);
    /// accent — deep toner cyan
    pub const ACCENT: Color32 = Color32::from_rgb(14, 116, 144);
    pub const ACCENT_SOFT: Color32 = Color32::from_rgb(207, 250, 254);

    pub const AMBER_BG: Color32 = Color32::from_rgb(254, 243, 199);
    pub const AMBER_TX: Color32 = Color32::from_rgb(146, 64, 14);
    pub const GREEN_BG: Color32 = Color32::from_rgb(220, 252, 231);
    pub const GREEN_TX: Color32 = Color32::from_rgb(22, 101, 52);
    pub const MAGENTA_BG: Color32 = Color32::from_rgb(252, 231, 243);
    pub const MAGENTA_TX: Color32 = Color32::from_rgb(157, 23, 77);
    pub const GRAY_BG: Color32 = Color32::from_rgb(243, 244, 246);
    pub const GRAY_TX: Color32 = Color32::from_rgb(107, 114, 128);

    /// process colors — the app's signature mark
    pub const CMYK: [Color32; 4] = [
        Color32::from_rgb(0, 174, 239),
        Color32::from_rgb(236, 0, 140),
        Color32::from_rgb(255, 212, 0),
        Color32::from_rgb(25, 28, 31),
    ];
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Debug)]
enum Status {
    Waiting,
    Printing,
    Done,
    Error(String),
}

impl Status {
    /// (badge text, text color, badge background)
    fn badge(&self) -> (String, Color32, Color32) {
        match self {
            Status::Waiting => ("Waiting".into(), theme::GRAY_TX, theme::GRAY_BG),
            Status::Printing => ("Printing…".into(), theme::AMBER_TX, theme::AMBER_BG),
            Status::Done => ("Sent".into(), theme::GREEN_TX, theme::GREEN_BG),
            Status::Error(_) => ("Error".into(), theme::MAGENTA_TX, theme::MAGENTA_BG),
        }
    }

    fn tooltip(&self) -> Option<String> {
        match self {
            Status::Done => Some("Sent to the printer".into()),
            Status::Error(e) => Some(format!("Error: {e}")),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct Job {
    path: PathBuf,
    name: String,
    status: Status,
}

impl Job {
    fn new(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Self {
            path,
            name,
            status: Status::Waiting,
        }
    }
}

type Queue = Arc<Mutex<Vec<Job>>>;

fn is_pdf(path: &std::path::Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

fn add_unique(queue: &mut Vec<Job>, path: PathBuf) {
    if !queue.iter().any(|j| j.path == path) {
        queue.push(Job::new(path));
    }
}

/// Moves the element at `src` into insertion slot `dst_slot` (0..=len,
/// exactly where the drop indicator line was shown).
fn apply_reorder<T>(v: &mut Vec<T>, src: usize, dst_slot: usize) {
    if src >= v.len() || dst_slot > v.len() {
        return;
    }
    let item = v.remove(src);
    let ins = if dst_slot > src { dst_slot - 1 } else { dst_slot };
    v.insert(ins.min(v.len()), item);
}

/// ETP_DEBUG=1 draws guide lines at row tops/bottoms/centers.
fn debug_geometry() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("ETP_DEBUG").is_ok())
}

// ---------------------------------------------------------------------------
// Print queue worker
// ---------------------------------------------------------------------------

fn run_queue(
    queue: Queue,
    engine: print::EngineKind,
    printer: Option<String>,
    settings: print::PrintSettings,
    stop: Arc<AtomicBool>,
) {
    loop {
        if stop.load(AtomicOrdering::Relaxed) {
            break;
        }
        let next = {
            let mut g = queue.lock().unwrap();
            g.iter_mut()
                .find(|j| j.status == Status::Waiting)
                .map(|j| {
                    j.status = Status::Printing;
                    j.path.clone()
                })
        };
        let Some(pdf) = next else { break };
        let result = print::print_pdf(&engine, &pdf, printer.as_deref(), &settings);
        {
            let mut g = queue.lock().unwrap();
            if let Some(j) = g
                .iter_mut()
                .find(|j| j.path == pdf && j.status == Status::Printing)
            {
                j.status = match result {
                    Ok(()) => Status::Done,
                    Err(e) => Status::Error(e),
                };
            }
        }
        let more_waiting = queue.lock().unwrap().iter().any(|j| j.status == Status::Waiting);
        if more_waiting {
            thread::sleep(PAUSE_BETWEEN_JOBS);
        }
    }
}

// ---------------------------------------------------------------------------
// System printers
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn list_printers() -> (Option<String>, Vec<String>) {
    use windows_sys::Win32::Graphics::Printing::{
        EnumPrintersW, GetDefaultPrinterW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
        PRINTER_INFO_4W,
    };
    unsafe {
        let flags: u32 = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        let mut needed: u32 = 0;
        let mut count: u32 = 0;
        EnumPrintersW(
            flags,
            std::ptr::null(),
            4,
            std::ptr::null_mut(),
            0,
            &mut needed,
            &mut count,
        );
        let mut printers = Vec::new();
        if needed > 0 {
            let mut buf: Vec<u8> = vec![0; needed as usize];
            let ok = EnumPrintersW(
                flags,
                std::ptr::null(),
                4,
                buf.as_mut_ptr(),
                needed,
                &mut needed,
                &mut count,
            );
            if ok != 0 {
                let base = buf.as_ptr() as *const PRINTER_INFO_4W;
                for i in 0..count as usize {
                    let info = base.add(i);
                    if !(*info).pPrinterName.is_null() {
                        printers.push(pwstr_to_string((*info).pPrinterName));
                    }
                }
            }
        }
        let mut default: Option<String> = None;
        let mut len: u32 = 0;
        GetDefaultPrinterW(std::ptr::null_mut(), &mut len);
        if len > 0 {
            let mut b: Vec<u16> = vec![0; len as usize];
            if GetDefaultPrinterW(b.as_mut_ptr(), &mut len) != 0 {
                default = Some(String::from_utf16_lossy(&b[..(len as usize).saturating_sub(1)]));
            }
        }
        (default, printers)
    }
}

#[cfg(windows)]
unsafe fn pwstr_to_string(p: *const u16) -> String {
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

#[cfg(not(windows))]
fn list_printers() -> (Option<String>, Vec<String>) {
    (None, Vec::new())
}

// ---------------------------------------------------------------------------
// Look & feel (light theme + system fonts)
// ---------------------------------------------------------------------------

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut insert = |name: &str, path: &str, family: FontFamily| {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                name.to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            if let Some(list) = fonts.families.get_mut(&family) {
                list.insert(0, name.to_owned());
            }
        }
    };
    insert("segoe-ui", "C:/Windows/Fonts/segoeui.ttf", FontFamily::Proportional);
    insert("consolas", "C:/Windows/Fonts/consola.ttf", FontFamily::Monospace);
    ctx.set_fonts(fonts);
}

fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::light();

    v.panel_fill = theme::PAPER;
    v.window_fill = theme::CARD;
    v.faint_bg_color = Color32::from_rgb(241, 240, 236);
    v.extreme_bg_color = theme::CARD;
    v.hyperlink_color = theme::ACCENT;
    v.error_fg_color = theme::MAGENTA_TX;
    v.warn_fg_color = theme::AMBER_TX;

    v.widgets.noninteractive.bg_fill = theme::CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, theme::HAIRLINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, theme::INK_SOFT);
    v.widgets.inactive.bg_fill = theme::CARD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, theme::HAIRLINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, theme::INK);
    v.widgets.hovered.bg_fill = theme::ACCENT_SOFT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, theme::ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, theme::ACCENT);
    v.widgets.active.bg_fill = theme::ACCENT_SOFT;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, theme::ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, theme::ACCENT);
    v.widgets.open.bg_fill = theme::CARD;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, theme::HAIRLINE);
    v.selection.bg_fill = theme::ACCENT;
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(7);
    }

    style.text_styles = [
        (TextStyle::Heading, FontId::new(21.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into_iter()
    .collect();
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);

    ctx.set_style(style);
}

/// Status badge: a rounded pastel pill with centered text.
fn badge(ui: &mut egui::Ui, text: &str, tx: Color32, bg: Color32) -> egui::Response {
    let font = FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, tx);
    let size = galley.size() + egui::vec2(16.0, 7.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same((size.y / 2.0) as u8), bg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, tx);
    resp
}

/// The four CMYK squares — the app's signature mark.
fn cmyk_mark(ui: &mut egui::Ui, size: f32) {
    let gap = size * 0.28;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(4.0 * size + 3.0 * gap, size),
        Sense::hover(),
    );
    let y = rect.center().y - size / 2.0;
    for (i, c) in theme::CMYK.iter().enumerate() {
        let x = rect.left() + i as f32 * (size + gap);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(size, size)),
            CornerRadius::same(2),
            *c,
        );
    }
}

/// Bordered settings section with a small caption.
fn settings_group(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    egui::Frame::default()
        .fill(theme::PAPER)
        .stroke(Stroke::new(1.0_f32, theme::HAIRLINE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(312.0);
            ui.label(
                RichText::new(title.to_uppercase())
                    .size(10.5)
                    .color(theme::INK_SOFT),
            );
            ui.add_space(4.0);
            content(ui);
        });
}

/// Round gear button (drawn by hand — no font dependency).
fn gear_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), Sense::click());
    let c = rect.center();
    let p = ui.painter();
    let hover = resp.hovered();
    if hover {
        p.circle_filled(c, 17.0, theme::GRAY_BG);
    }
    let color = if hover { theme::INK } else { theme::INK_SOFT };
    let r_out = 8.5_f32;
    let r_in = 6.2_f32;
    let step = std::f32::consts::TAU / 8.0;
    let tooth = step * 0.30;
    let root = step * 0.50;
    let pt = |ang: f32, r: f32| egui::pos2(c.x + ang.cos() * r, c.y + ang.sin() * r);
    let mut pts = Vec::with_capacity(32);
    for k in 0..8 {
        let a = k as f32 * step;
        pts.push(pt(a - root, r_in));
        pts.push(pt(a - tooth, r_out));
        pts.push(pt(a + tooth, r_out));
        pts.push(pt(a + root, r_in));
    }
    p.add(egui::Shape::convex_polygon(
        pts,
        Color32::TRANSPARENT,
        Stroke::new(1.4_f32, color),
    ));
    p.circle_stroke(c, 3.2, Stroke::new(1.4_f32, color));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Settings")
}

/// Progress bar drawn by hand: 26 px tall, accent fill, text centered both
/// ways — white over the fill, ink over the empty track.
fn progress_bar(ui: &mut egui::Ui, frac: f32, text: &str) {
    let h = 26.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let cr = CornerRadius::same(8);
    p.rect_filled(rect, cr, Color32::from_rgb(226, 232, 240));
    let frac = frac.clamp(0.0, 1.0);
    let fill = if rect.width() * frac >= 16.0 {
        Some(egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + rect.width() * frac, rect.bottom()),
        ))
    } else {
        None
    };
    if let Some(f) = fill {
        p.rect_filled(f, cr, theme::ACCENT);
    }
    let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(12.5), theme::INK);
    let pos = rect.center() - galley.size() / 2.0;
    // ink over the unfilled track, white over the accent fill
    let rest = egui::Rect::from_min_max(
        egui::pos2(rect.left() + rect.width() * frac, rect.top()),
        rect.max,
    );
    p.with_clip_rect(rest).galley(pos, galley.clone(), theme::INK);
    if let Some(f) = fill {
        p.with_clip_rect(f)
            .galley_with_override_text_color(pos, galley, Color32::WHITE);
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    queue: Queue,
    worker: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    printer: String,
    printers: Vec<String>,
    default_printer: Option<String>,
    autostart: bool,
    settings_open: bool,
    /// ongoing row drag: (source index, insertion index)
    drag: Option<(usize, usize)>,
    engine: Option<print::EngineKind>,
    pset: print::PrintSettings,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        install_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx);
        let (default_printer, printers) = list_printers();
        let printer = if printers.iter().any(|p| p == DEFAULT_PRINTER) {
            DEFAULT_PRINTER.to_string()
        } else {
            default_printer.clone().unwrap_or_default()
        };
        let queue: Queue = Arc::new(Mutex::new(Vec::new()));
        // hidden demo mode (ETP_DEMO=1) — UI preview with a fake queue
        if std::env::var("ETP_DEMO").is_ok() {
            let demo: &[(&str, Status)] = &[
                ("invoice_2026-10-01_kowalski.pdf", Status::Done),
                ("invoice_2026-10-02_nowak.pdf", Status::Done),
                (
                    "contract_2026_final_v3_withAVeryLongFileNameThatWillCut.pdf",
                    Status::Printing,
                ),
                ("delivery_report.pdf", Status::Waiting),
                ("invoice_10.pdf", Status::Waiting),
                ("invoice_2.pdf", Status::Waiting),
                (
                    "damaged.pdf",
                    Status::Error("failed to launch (demo)".into()),
                ),
            ];
            *queue.lock().unwrap() = demo
                .iter()
                .map(|(n, s)| Job {
                    path: PathBuf::from(format!("C:/demo/{n}")),
                    name: (*n).to_string(),
                    status: s.clone(),
                })
                .collect();
        }
        Self {
            queue,
            worker: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            printer,
            printers,
            default_printer,
            autostart: false,
            settings_open: std::env::var("ETP_DEMO").as_deref() == Ok("2"),
            drag: None,
            engine: print::EngineKind::detect(),
            pset: print::PrintSettings::default(),
        }
    }

    fn printing(&self) -> bool {
        self.worker.is_some()
    }

    fn start_worker(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let Some(engine) = self.engine.clone() else { return };
        let printer = self.printer.trim().to_owned();
        let printer = if printer.is_empty() { None } else { Some(printer) };
        let queue = self.queue.clone();
        let stop = self.stop_flag.clone();
        let settings = self.pset;
        self.stop_flag.store(false, AtomicOrdering::Relaxed);
        self.worker = Some(thread::spawn(move || {
            run_queue(queue, engine, printer, settings, stop)
        }));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. New files from drag & drop
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .filter(|p| is_pdf(p))
                .collect()
        });
        let files_added = !dropped.is_empty();
        if files_added {
            let mut q = self.queue.lock().unwrap();
            for p in dropped {
                add_unique(&mut q, p);
            }
        }

        // 2. Reap a finished worker thread
        if self.worker.as_ref().map(|h| h.is_finished()).unwrap_or(false) {
            if let Some(h) = self.worker.take() {
                let _ = h.join();
            }
        }

        if self.printing() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }

        // 3. Queue snapshot for drawing (short lock)
        let snapshot: Vec<(PathBuf, String, Status)> = {
            let q = self.queue.lock().unwrap();
            q.iter()
                .map(|j| (j.path.clone(), j.name.clone(), j.status.clone()))
                .collect()
        };
        let total = snapshot.len();
        let waiting = snapshot
            .iter()
            .filter(|(_, _, s)| *s == Status::Waiting)
            .count();
        let done = snapshot
            .iter()
            .filter(|(_, _, s)| *s == Status::Done)
            .count();
        let errors = snapshot
            .iter()
            .filter(|(_, _, s)| matches!(s, Status::Error(_)))
            .count();

        let mut files_picked = false;
        let mut action_clear = false;
        let mut action_start = false;
        let mut remove_path: Option<PathBuf> = None;
        let mut open_settings = false;

        let hovered_files: Vec<String> = ctx.input(|i| {
            i.raw
                .hovered_files
                .iter()
                .filter_map(|f| f.path.clone())
                .map(|p| {
                    p.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect()
        });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(theme::PAPER)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                // ---- Header: CMYK + title, counter and gear to the right ----
                ui.horizontal(|ui| {
                    cmyk_mark(ui, 8.0);
                    ui.add_space(4.0);
                    ui.label(RichText::new("EasyToPrint").size(21.0).color(theme::INK));
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        if gear_button(ui).clicked() {
                            open_settings = true;
                        }
                        if total > 0 {
                            let chip = if total == 1 {
                                "1 file".to_string()
                            } else {
                                format!("{total} files")
                            };
                            badge(ui, &chip, theme::INK_SOFT, theme::GRAY_BG);
                        }
                    });
                });
                ui.add_space(10.0);

                // ---- Drop zone: a sheet of paper on a stack ----
                let dragging = !hovered_files.is_empty();
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 195.0),
                    Sense::click(),
                );
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                let hov = resp.hovered();
                let active = dragging || hov;
                let p = ui.painter();
                let cr = CornerRadius::same(12);
                p.rect_stroke(
                    rect.translate(egui::vec2(7.0, 7.0)),
                    cr,
                    Stroke::new(1.0_f32, theme::HAIRLINE),
                    StrokeKind::Middle,
                );
                p.rect_stroke(
                    rect.translate(egui::vec2(3.5, 3.5)),
                    cr,
                    Stroke::new(1.0_f32, theme::HAIRLINE),
                    StrokeKind::Middle,
                );
                p.rect_filled(
                    rect,
                    cr,
                    if hov && !dragging {
                        Color32::from_rgb(240, 249, 252)
                    } else {
                        theme::CARD
                    },
                );
                if dragging {
                    p.rect_stroke(rect, cr, Stroke::new(2.0_f32, theme::ACCENT), StrokeKind::Middle);
                    // process-color strip on the edge of the sheet
                    let top = rect.top() + 14.0;
                    let seg = (rect.height() - 28.0) / 4.0;
                    for (i, c) in theme::CMYK.iter().enumerate() {
                        p.rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(rect.left() + 12.0, top + i as f32 * seg),
                                egui::vec2(4.0, seg - 4.0),
                            ),
                            CornerRadius::same(1),
                            *c,
                        );
                    }
                } else if hov {
                    p.rect_stroke(
                        rect,
                        cr,
                        Stroke::new(1.5_f32, Color32::from_rgb(120, 190, 210)),
                        StrokeKind::Middle,
                    );
                } else {
                    p.rect_stroke(rect, cr, Stroke::new(1.0_f32, theme::HAIRLINE), StrokeKind::Middle);
                }
                let c = rect.center();
                let shift = if dragging { 8.0 } else { 0.0 };
                p.circle_filled(
                    egui::pos2(c.x, c.y - 50.0),
                    16.0,
                    if active { theme::ACCENT_SOFT } else { theme::GRAY_BG },
                );
                p.text(
                    egui::pos2(c.x, c.y - 50.0),
                    Align2::CENTER_CENTER,
                    "↓",
                    FontId::proportional(17.0),
                    if active { theme::ACCENT } else { theme::INK_SOFT },
                );
                p.text(
                    egui::pos2(c.x + shift, c.y + 12.0),
                    Align2::CENTER_CENTER,
                    if dragging {
                        "Drop to add to the queue"
                    } else {
                        "Drag & drop PDF files"
                    },
                    FontId::proportional(16.0),
                    theme::INK,
                );
                let sub = if dragging {
                    let names = hovered_files.join(", ");
                    let names: String = names.chars().take(60).collect();
                    format!("Add: {names}")
                } else {
                    "or click here to browse files".to_string()
                };
                p.text(
                    egui::pos2(c.x + shift, c.y + 46.0),
                    Align2::CENTER_CENTER,
                    sub,
                    FontId::proportional(12.0),
                    if dragging { theme::ACCENT } else { theme::INK_SOFT },
                );
                if resp.clicked() {
                    if let Some(paths) = rfd::FileDialog::new()
                        .add_filter("PDF file", &["pdf"])
                        .pick_files()
                    {
                        let mut q = self.queue.lock().unwrap();
                        for p in paths.into_iter().filter(|p| is_pdf(p)) {
                            add_unique(&mut q, p);
                        }
                        files_picked = true;
                    }
                }
                ui.add_space(12.0);

                // ---- Buttons ----
                ui.horizontal(|ui| {
                    if self.printing() {
                        ui.add_enabled(
                            false,
                            egui::Button::new(
                                RichText::new("Printing…")
                                    .color(Color32::WHITE)
                                    .size(14.0),
                            )
                            .fill(theme::ACCENT)
                            .corner_radius(CornerRadius::same(9))
                            .min_size(egui::vec2(0.0, 36.0)),
                        );
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Stop")
                                        .color(theme::MAGENTA_TX)
                                        .size(13.5),
                                )
                                .corner_radius(CornerRadius::same(9))
                                .min_size(egui::vec2(0.0, 36.0)),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.stop_flag.store(true, AtomicOrdering::Relaxed);
                        }
                    } else {
                        let label = if waiting > 0 {
                            format!("Print ({waiting})")
                        } else {
                            "Print".to_string()
                        };
                        if ui
                            .add_enabled(
                                waiting > 0 && self.engine.is_some(),
                                egui::Button::new(
                                    RichText::new(label).color(Color32::WHITE).size(14.0),
                                )
                                .fill(theme::ACCENT)
                                .corner_radius(CornerRadius::same(9))
                                .min_size(egui::vec2(0.0, 36.0)),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            action_start = true;
                        }
                    }
                    if ui
                        .add_enabled(
                            total > 0,
                            egui::Button::new(RichText::new("Clear").size(13.5).color(theme::INK))
                                .corner_radius(CornerRadius::same(9))
                                .min_size(egui::vec2(0.0, 36.0)),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        action_clear = true;
                    }
                });

                if self.engine.is_none() {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(
                            "No print engine — place pdfium.dll in the same folder as the program.",
                        )
                        .size(12.0)
                        .color(theme::MAGENTA_TX),
                    );
                }

                // ---- Progress ----
                if total > 0 {
                    ui.add_space(6.0);
                    let fin = done + errors;
                    let frac = fin as f32 / total as f32;
                    let mut text = format!("Sent {fin} of {total}");
                    if errors > 0 {
                        text.push_str(&format!(" · errors: {errors}"));
                    }
                    progress_bar(ui, frac, &text);
                    if let Some((_, name, _)) = snapshot
                        .iter()
                        .find(|(_, _, s)| *s == Status::Printing)
                    {
                        ui.label(
                            RichText::new(format!("Printing: {name}"))
                                .size(12.0)
                                .color(theme::AMBER_TX),
                        );
                    }
                }

                // ---- Queue table ----
                ui.add_space(10.0);
                egui::Frame::default()
                    .fill(theme::CARD)
                    .stroke(Stroke::new(1.0_f32, theme::HAIRLINE))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        if total == 0 {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 170.0),
                                Sense::hover(),
                            );
                            let c = rect.center();
                            ui.painter().text(
                                c,
                                Align2::CENTER_CENTER,
                                "The queue is empty",
                                FontId::proportional(14.0),
                                theme::INK_SOFT,
                            );
                            ui.painter().text(
                                egui::pos2(c.x, c.y + 24.0),
                                Align2::CENTER_CENTER,
                                "drop PDF files above to get started",
                                FontId::proportional(12.0),
                                theme::INK_SOFT,
                            );
                        } else {
                            // table header
                            let (hr, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), HEAD_H),
                                Sense::hover(),
                            );
                            let p = ui.painter();
                            p.text(
                                egui::pos2(hr.left() + 38.0, hr.center().y),
                                Align2::RIGHT_CENTER,
                                "#",
                                FontId::proportional(11.0),
                                theme::INK_SOFT,
                            );
                            p.text(
                                egui::pos2(hr.left() + 54.0, hr.center().y),
                                Align2::LEFT_CENTER,
                                "File",
                                FontId::proportional(11.0),
                                theme::INK_SOFT,
                            );
                            p.text(
                                egui::pos2(hr.right() - 58.0, hr.center().y),
                                Align2::RIGHT_CENTER,
                                "Status",
                                FontId::proportional(11.0),
                                theme::INK_SOFT,
                            );
                            p.line_segment(
                                [
                                    egui::pos2(hr.left() + 4.0, hr.bottom() - 1.0),
                                    egui::pos2(hr.right() - 4.0, hr.bottom() - 1.0),
                                ],
                                Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgb(216, 214, 208),
                                ),
                            );

                            let mut rm: Option<usize> = None;
                            let mut dragging_now: Option<(usize, usize)> = None;
                            let mut first_top: Option<f32> = None;
                            let debug_lines = debug_geometry();
                            ui.spacing_mut().item_spacing.y = ROW_GAP;
                            ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = ROW_GAP;
                                    for i in 0..total {
                                        let (path, name, status) = &snapshot[i];
                                        // egui allocates the exact row; its rect is the
                                        // single source of truth for everything below
                                        let (cell, _) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), ROW_H),
                                            Sense::hover(),
                                        );
                                        if first_top.is_none() {
                                            first_top = Some(cell.top());
                                        }
                                        // hover + drag on the whole cell
                                        let resp = ui.interact(
                                            cell,
                                            egui::Id::new(("row", path)),
                                            Sense::click_and_drag(),
                                        );
                                        let hovered = ui.rect_contains_pointer(cell);
                                        let is_dragged = resp.dragged();
                                        if is_dragged {
                                            if let Some(pos) = resp.interact_pointer_pos() {
                                                if let Some(top) = first_top {
                                                    let dst =
                                                        (((pos.y - top + ROW_GAP / 2.0) / STRIDE)
                                                            .floor() as i64)
                                                            .clamp(0, total as i64)
                                                            as usize;
                                                    dragging_now = Some((i, dst));
                                                }
                                            }
                                        }
                                        // cell background — the hover field IS the cell
                                        let bg = match status {
                                            Status::Printing => {
                                                Some(Color32::from_rgb(255, 246, 221))
                                            }
                                            _ if is_dragged => Some(theme::ACCENT_SOFT),
                                            _ if hovered => {
                                                Some(Color32::from_rgb(244, 243, 239))
                                            }
                                            _ => None,
                                        };
                                        if let Some(bg) = bg {
                                            ui.painter()
                                                .rect_filled(cell, CornerRadius::same(6), bg);
                                        }
                                        if debug_lines {
                                            let p = ui.painter();
                                            let thin = Stroke::new(1.0_f32, Color32::RED);
                                            p.line_segment(
                                                [
                                                    cell.left_top(),
                                                    egui::pos2(cell.right(), cell.top()),
                                                ],
                                                thin,
                                            );
                                            p.line_segment(
                                                [
                                                    egui::pos2(cell.left(), cell.bottom()),
                                                    cell.right_bottom(),
                                                ],
                                                Stroke::new(1.0_f32, Color32::BLUE),
                                            );
                                            p.line_segment(
                                                [
                                                    egui::pos2(cell.left(), cell.center().y),
                                                    egui::pos2(cell.right(), cell.center().y),
                                                ],
                                                Stroke::new(1.0_f32, Color32::GREEN),
                                            );
                                        }
                                        let cy = cell.center().y;
                                        // row number (+1 px optical: digits have no
                                        // descenders, the line box sits low)
                                        ui.painter().text(
                                            egui::pos2(cell.left() + 38.0, cy + 1.0),
                                            Align2::RIGHT_CENTER,
                                            format!("{}", i + 1),
                                            FontId::monospace(11.5),
                                            theme::INK_SOFT,
                                        );
                                        // file name (+1.5 px optical nudge: centers the
                                        // glyphs, not the line box)
                                        let name_rect = egui::Rect::from_min_max(
                                            egui::pos2(cell.left() + 54.0, cell.top()),
                                            egui::pos2(cell.right() - RIGHT_ZONE, cell.bottom()),
                                        );
                                        let font = FontId::monospace(12.5);
                                        let mut gal = ui
                                            .painter()
                                            .layout_no_wrap(name.as_str().to_owned(), font.clone(), theme::INK);
                                        if gal.size().x > name_rect.width() {
                                            let chars: Vec<char> = name.chars().collect();
                                            let mut cut = chars.len();
                                            while cut > 1 {
                                                cut -= 1;
                                                let probe: String =
                                                    chars[..cut].iter().collect::<String>() + "…";
                                                gal = ui.painter().layout_no_wrap(probe, font.clone(), theme::INK);
                                                if gal.size().x <= name_rect.width() {
                                                    break;
                                                }
                                            }
                                        }
                                        ui.painter().galley(
                                            egui::pos2(
                                                name_rect.left(),
                                                cy - gal.size().y / 2.0 + 1.5,
                                            ),
                                            gal,
                                            theme::INK,
                                        );
                                        ui.interact(
                                            name_rect,
                                            egui::Id::new(("name", path)),
                                            Sense::hover(),
                                        )
                                        .on_hover_text(path.display().to_string());
                                        // status badge
                                        let (txt, tx, bc) = status.badge();
                                        let galley = ui
                                            .painter()
                                            .layout_no_wrap(txt, FontId::proportional(11.0), tx);
                                        let bs = galley.size() + egui::vec2(16.0, 7.0);
                                        let brect = egui::Rect::from_min_size(
                                            egui::pos2(cell.right() - 46.0 - bs.x, cy - bs.y / 2.0),
                                            bs,
                                        );
                                        ui.painter().rect_filled(
                                            brect,
                                            CornerRadius::same((bs.y / 2.0) as u8),
                                            bc,
                                        );
                                        ui.painter().galley(
                                            brect.center() - galley.size() / 2.0,
                                            galley,
                                            tx,
                                        );
                                        if let Some(t) = status.tooltip() {
                                            ui.interact(
                                                brect,
                                                egui::Id::new(("badge", path)),
                                                Sense::hover(),
                                            )
                                            .on_hover_text(t);
                                        }
                                        // remove control: borderless red ×,
                                        // soft red circle + hand cursor on hover
                                        if *status != Status::Printing {
                                            let xr = egui::Rect::from_center_size(
                                                egui::pos2(cell.right() - 21.0, cy),
                                                egui::vec2(28.0, 28.0),
                                            );
                                            let resp = ui.interact(
                                                xr,
                                                egui::Id::new(("remove", path)),
                                                Sense::click(),
                                            );
                                            let hov = resp.hovered();
                                            if hov {
                                                ui.painter().circle_filled(
                                                    xr.center(),
                                                    12.0,
                                                    theme::MAGENTA_BG,
                                                );
                                            }
                                            ui.painter().text(
                                                xr.center(),
                                                Align2::CENTER_CENTER,
                                                "×",
                                                FontId::proportional(15.0),
                                                if hov { theme::MAGENTA_TX } else { theme::INK_SOFT },
                                            );
                                            let resp = resp
                                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                                            if resp.clicked() {
                                                rm = Some(i);
                                            }
                                        }
                                        // separator in the MIDDLE of the gap below the
                                        // cell — symmetric margins by construction
                                        if i + 1 < total {
                                            let y = cell.bottom() + ROW_GAP / 2.0;
                                            ui.painter().line_segment(
                                                [
                                                    egui::pos2(cell.left() + 14.0, y),
                                                    egui::pos2(cell.right() - 14.0, y),
                                                ],
                                                Stroke::new(1.0_f32, theme::HAIRLINE),
                                            );
                                        }
                                    }
                                    // drop indicator — drawn once, after all rows,
                                    // exactly at the slot the pointer maps to
                                    if let (Some((_, dst)), Some(top)) =
                                        (dragging_now, first_top)
                                    {
                                        let y = top + dst as f32 * STRIDE - ROW_GAP / 2.0;
                                        let l = ui.min_rect().left() + 8.0;
                                        let r = ui.min_rect().right() - 8.0;
                                        ui.painter().line_segment(
                                            [egui::pos2(l, y), egui::pos2(r, y)],
                                            Stroke::new(2.5_f32, theme::ACCENT),
                                        );
                                    }
                                });
                            if let Some(i) = rm {
                                remove_path = Some(snapshot[i].0.clone());
                            }
                            // finish a row drag: move the job to the drop slot
                            if let Some((s, d)) = dragging_now {
                                self.drag = Some((s, d));
                            } else if let Some((s, d)) = self.drag.take() {
                                apply_reorder(&mut self.queue.lock().unwrap(), s, d);
                            }
                        }
                    });
            });

        // 4. Settings window (gear button)
        if open_settings {
            self.settings_open = true;
        }
        if self.settings_open {
            let screen = ctx.content_rect();
            let pos = egui::pos2(
                (screen.right() - 400.0).max(screen.left() + 16.0),
                screen.top() + 70.0,
            );
            let mut open = self.settings_open;
            let mut close = false;
            // exact window height for the content: base + optional lines
            let mut h = 404.0;
            if self.printing() {
                h += 17.0;
            }
            if matches!(
                self.engine,
                Some(print::EngineKind::PdfToPrinter(_))
            ) {
                h += 17.0;
            }
            let win_frame = egui::Frame::window(&ctx.style())
                .inner_margin(egui::Margin::same(14))
                .corner_radius(CornerRadius::same(10))
                .stroke(Stroke::new(1.0_f32, theme::HAIRLINE))
                .fill(theme::CARD);
            egui::Window::new("Settings")
                .id(egui::Id::new("settings-window"))
                .default_pos(pos)
                .fixed_size([390.0, h])
                .frame(win_frame)
                .resizable(false)
                .collapsible(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.set_width(336.0);
                    // ---- General ----
                    settings_group(ui, "General", |ui| {
                        ui.label(
                            RichText::new("Printer")
                                .size(12.5)
                                .color(theme::INK_SOFT),
                        );
                        let selected = if self.printer.is_empty() {
                            match &self.default_printer {
                                Some(d) => format!("Default ({d})"),
                                None => "Default".to_string(),
                            }
                        } else {
                            self.printer.clone()
                        };
                        ui.add_enabled_ui(!self.printing(), |ui| {
                            egui::ComboBox::from_id_salt("printer")
                                .selected_text(
                                    RichText::new(selected).size(13.0).color(theme::INK),
                                )
                                .width(312.0)
                                .show_ui(ui, |ui| {
                                    let def_label = match &self.default_printer {
                                        Some(d) => format!("Default ({d})"),
                                        None => "Default".to_string(),
                                    };
                                    ui.selectable_value(&mut self.printer, String::new(), def_label);
                                    for p in self.printers.clone() {
                                        ui.selectable_value(&mut self.printer, p.clone(), p);
                                    }
                                });
                        });
                        if self.printing() {
                            ui.label(
                                RichText::new("applies from the next print job")
                                    .size(11.5)
                                    .color(theme::AMBER_TX),
                            );
                        }
                        ui.add_space(6.0);
                        ui.checkbox(
                            &mut self.autostart,
                            RichText::new("Print immediately after adding files")
                                .size(13.0)
                                .color(theme::INK),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    });
                    // ---- Printing ----
                    settings_group(ui, "Printing", |ui| {
                        ui.label(RichText::new("Duplex").size(12.5).color(theme::INK_SOFT));
                        egui::ComboBox::from_id_salt("duplex")
                            .selected_text(
                                RichText::new(self.pset.duplex.label())
                                    .size(13.0)
                                    .color(theme::INK),
                            )
                            .width(312.0)
                            .show_ui(ui, |ui| {
                                for mode in [
                                    print::DuplexMode::Printer,
                                    print::DuplexMode::Simplex,
                                    print::DuplexMode::LongEdge,
                                    print::DuplexMode::ShortEdge,
                                ] {
                                    ui.selectable_value(&mut self.pset.duplex, mode, mode.label());
                                }
                            });
                        ui.add_space(6.0);
                        ui.label(RichText::new("Color").size(12.5).color(theme::INK_SOFT));
                        egui::ComboBox::from_id_salt("color")
                            .selected_text(
                                RichText::new(self.pset.color.label())
                                    .size(13.0)
                                    .color(theme::INK),
                            )
                            .width(312.0)
                            .show_ui(ui, |ui| {
                                for mode in [
                                    print::ColorMode::Printer,
                                    print::ColorMode::Color,
                                    print::ColorMode::Grayscale,
                                ] {
                                    ui.selectable_value(&mut self.pset.color, mode, mode.label());
                                }
                            });
                        ui.add_space(6.0);
                        ui.label(RichText::new("Paper size").size(12.5).color(theme::INK_SOFT));
                        egui::ComboBox::from_id_salt("paper")
                            .selected_text(
                                RichText::new(self.pset.paper.label())
                                    .size(13.0)
                                    .color(theme::INK),
                            )
                            .width(312.0)
                            .show_ui(ui, |ui| {
                                for mode in [
                                    print::PaperMode::Printer,
                                    print::PaperMode::A4,
                                    print::PaperMode::Letter,
                                ] {
                                    ui.selectable_value(&mut self.pset.paper, mode, mode.label());
                                }
                            });
                    });
                    // ---- footer ----
                    ui.add_space(6.0);
                    if let Some(engine) = &self.engine {
                        ui.label(
                            RichText::new(format!("Print engine: {}", engine.label()))
                                .size(11.5)
                                .color(theme::INK_SOFT),
                        );
                        if matches!(engine, print::EngineKind::PdfToPrinter(_)) {
                            ui.label(
                                RichText::new("print options need the built-in engine")
                                    .size(11.5)
                                    .color(theme::AMBER_TX),
                            );
                        }
                    }
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Close").size(13.0).color(theme::INK),
                                )
                                .corner_radius(CornerRadius::same(8))
                                .min_size(egui::vec2(88.0, 28.0)),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
            self.settings_open = open && !close;
        }

        // 5. Actions after drawing (no lock held during UI)
        if action_clear {
            self.queue
                .lock()
                .unwrap()
                .retain(|j| j.status == Status::Printing);
        }
        if let Some(p) = remove_path {
            self.queue
                .lock()
                .unwrap()
                .retain(|j| j.path != p || j.status == Status::Printing);
        }
        if action_start || ((files_added || files_picked) && self.autostart) {
            self.start_worker();
        }
    }
}

fn main() -> eframe::Result<()> {
    let icon = load_icon();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("EasyToPrint — bulk PDF printing")
        .with_inner_size([620.0, 830.0])
        .with_min_inner_size([480.0, 560.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "EasyToPrint",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// Window/taskbar icon — decoded from the same PNG the exe icon is built from.
fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon_256.png"))
        .ok()?
        .into_rgba8();
    let (w, h) = (img.width(), img.height());
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_dedups_by_path() {
        let mut q = Vec::new();
        add_unique(&mut q, PathBuf::from("C:/a.pdf"));
        add_unique(&mut q, PathBuf::from("C:/a.pdf"));
        add_unique(&mut q, PathBuf::from("C:/b.pdf"));
        assert_eq!(q.len(), 2);
        assert!(q.iter().all(|j| j.status == Status::Waiting));
    }

    #[test]
    fn reorder_moves_elements_correctly() {
        // drag first to the very end
        let mut v = vec![10, 20, 30];
        apply_reorder(&mut v, 0, 3);
        assert_eq!(v, vec![20, 30, 10]);
        // drag first between rows 1 and 2
        let mut v = vec![10, 20, 30];
        apply_reorder(&mut v, 0, 2);
        assert_eq!(v, vec![20, 10, 30]);
        // drag last to the front
        let mut v = vec![10, 20, 30];
        apply_reorder(&mut v, 2, 0);
        assert_eq!(v, vec![30, 10, 20]);
        // drop back onto the same slot — no change
        let mut v = vec![10, 20, 30];
        apply_reorder(&mut v, 1, 1);
        assert_eq!(v, vec![10, 20, 30]);
        apply_reorder(&mut v, 1, 2);
        assert_eq!(v, vec![10, 20, 30]);
        // single element, clamped destination
        let mut v = vec![10];
        apply_reorder(&mut v, 0, 1);
        assert_eq!(v, vec![10]);
        // out-of-range source/destination are ignored
        let mut v = vec![10, 20];
        apply_reorder(&mut v, 5, 0);
        apply_reorder(&mut v, 0, 9);
        assert_eq!(v, vec![10, 20]);
    }
}
