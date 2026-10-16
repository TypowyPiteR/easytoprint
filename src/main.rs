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

mod config;
mod lang;
mod print;
mod shell;

use config::Config;
use lang::Lang;

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
    /// (translation key, text color, badge background)
    fn badge(&self) -> (&'static str, Color32, Color32) {
        match self {
            Status::Waiting => ("status.waiting", theme::GRAY_TX, theme::GRAY_BG),
            Status::Printing => ("status.printing", theme::AMBER_TX, theme::AMBER_BG),
            Status::Done => ("status.sent", theme::GREEN_TX, theme::GREEN_BG),
            Status::Error(_) => ("status.error", theme::MAGENTA_TX, theme::MAGENTA_BG),
        }
    }

    fn tooltip_key(&self) -> Option<&'static str> {
        match self {
            Status::Done => Some("status.sent.tip"),
            Status::Error(_) => Some("status.error.tip"),
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

/// A removed queue row, playing its fade-out animation.
#[derive(Clone, Debug)]
struct Ghost {
    name: String,
    status: Status,
    key: u64,
}

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

/// The four CMYK squares in a 2×2 grid — the app's signature mark
/// (identical to the application icon).
fn cmyk_mark(ui: &mut egui::Ui, size: f32) {
    let gap = size * 0.22;
    let total = 2.0 * size + gap;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, total), Sense::hover());
    let x0 = rect.left();
    let y0 = rect.top();
    for (i, c) in theme::CMYK.iter().enumerate() {
        let x = x0 + (i as f32 % 2.0) * (size + gap);
        let y = y0 + ((i as f32 / 2.0).floor()) * (size + gap);
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
fn gear_button(ui: &mut egui::Ui, tooltip: &str) -> egui::Response {
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
        .on_hover_text(tooltip)
}

/// Replaces `{name}` placeholders in a translated template.
fn fill(template: &str, name: &str, value: &str) -> String {
    template.replace(&format!("{{{name}}}"), value)
}

/// Linear blend of two colors.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        ch(a.r(), b.r()),
        ch(a.g(), b.g()),
        ch(a.b(), b.b()),
        ch(a.a(), b.a()),
    )
}

/// Color with reduced alpha (for fade-in animations).
fn faded(c: Color32, t: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * t) as u8)
}

/// Custom window title bar (native decorations are off).
/// Drawn edge-to-edge by the caller (TopBottomPanel with zero margins).
fn title_bar(ui: &mut egui::Ui, title: &str, corner: u8) {
    let h = 40.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), h),
        Sense::click_and_drag(),
    );
    let p = ui.painter();
    p.line_segment(
        [rect.left_bottom(), egui::pos2(rect.right(), rect.bottom())],
        Stroke::new(1.0_f32, theme::HAIRLINE),
    );
    p.text(
        egui::pos2(rect.left() + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(12.5),
        theme::INK,
    );
    // window buttons: minimize, maximize, close (right to left)
    let btn_w = 42.0;
    for (i, kind) in ['m', 'z', 'x'].iter().enumerate() {
        let x1 = rect.right() - (3 - i) as f32 * btn_w;
        let br = egui::Rect::from_min_max(
            egui::pos2(x1, rect.top()),
            egui::pos2(x1 + btn_w, rect.bottom()),
        );
        let bid = egui::Id::new(("winbtn", kind));
        let bresp = ui.interact(br, bid, Sense::click());
        let hov = bresp.hovered();
        let center = br.center();
        match kind {
            'm' => {
                if hov {
                    p.rect_filled(br, CornerRadius::same(0), theme::GRAY_BG);
                }
                let col = if hov { theme::INK } else { theme::INK_SOFT };
                p.line_segment(
                    [egui::pos2(center.x - 5.0, center.y), egui::pos2(center.x + 5.0, center.y)],
                    Stroke::new(1.4_f32, col),
                );
                if bresp.clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            }
            'z' => {
                if hov {
                    p.rect_filled(br, CornerRadius::same(0), theme::GRAY_BG);
                }
                let col = if hov { theme::INK } else { theme::INK_SOFT };
                p.rect_stroke(
                    egui::Rect::from_center_size(center, egui::vec2(10.0, 10.0)),
                    CornerRadius::same(1),
                    Stroke::new(1.3_f32, col),
                    StrokeKind::Middle,
                );
                if bresp.clicked() {
                    let maximized = ui
                        .ctx()
                        .input(|i| i.viewport().maximized.unwrap_or(false));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
            }
            _ => {
                if hov {
                    // rounded to match the window corner (no square poking out)
                    p.rect_filled(
                        br,
                        CornerRadius {
                            nw: 0,
                            ne: corner,
                            sw: 0,
                            se: 0,
                        },
                        Color32::from_rgb(232, 17, 35),
                    );
                }
                let col = if hov { Color32::WHITE } else { theme::INK_SOFT };
                // same visual weight as the other buttons: two 1.4 px strokes
                let s = 5.0;
                p.line_segment(
                    [egui::pos2(center.x - s, center.y - s), egui::pos2(center.x + s, center.y + s)],
                    Stroke::new(1.4_f32, col),
                );
                p.line_segment(
                    [egui::pos2(center.x - s, center.y + s), egui::pos2(center.x + s, center.y - s)],
                    Stroke::new(1.4_f32, col),
                );
                if bresp.clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
    // drag to move; double-click toggles maximize
    if resp.dragged() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

/// All visible strings for one frame, translated up front.
struct UiTexts {
    files_one: String,
    drop_main: String,
    drop_sub: String,
    drop_active: String,
    btn_print: String,
    btn_printing: String,
    btn_stop: String,
    btn_clear: String,
    warn_engine: String,
    empty_title: String,
    empty_sub: String,
    col_file: String,
    col_status: String,
    st_waiting: String,
    st_printing: String,
    st_sent: String,
    st_error: String,
    st_sent_tip: String,
    set_title: String,
    grp_general: String,
    grp_printing: String,
    lbl_language: String,
    lbl_printer: String,
    def_printer: String,
    lbl_duplex: String,
    lbl_color: String,
    lbl_paper: String,
    paper_default: String,
    paper_auto: String,
    legacy_note: String,
    close: String,
    gear_tip: String,
    next_job_hint: String,
    autostart: String,
    explorer_menu: String,
    // templates with placeholders
    files_many_tpl: String,
    print_n_tpl: String,
    drop_add_tpl: String,
    prog_sent_tpl: String,
    prog_errors_tpl: String,
    prog_now_tpl: String,
    err_tip_tpl: String,
    def_printer_named_tpl: String,
    duplex_labels: [String; 4],
    color_labels: [String; 3],
}

impl UiTexts {
    fn build(lang: &Lang) -> UiTexts {
        UiTexts {
            files_one: lang.tr("files.one"),
            drop_main: lang.tr("drop.main"),
            drop_sub: lang.tr("drop.sub"),
            drop_active: lang.tr("drop.active"),
            btn_print: lang.tr("btn.print"),
            btn_printing: lang.tr("btn.printing"),
            btn_stop: lang.tr("btn.stop"),
            btn_clear: lang.tr("btn.clear"),
            warn_engine: lang.tr("warn.engine"),
            empty_title: lang.tr("queue.empty.title"),
            empty_sub: lang.tr("queue.empty.sub"),
            col_file: lang.tr("queue.col.file"),
            col_status: lang.tr("queue.col.status"),
            st_waiting: lang.tr("status.waiting"),
            st_printing: lang.tr("status.printing"),
            st_sent: lang.tr("status.sent"),
            st_error: lang.tr("status.error"),
            st_sent_tip: lang.tr("status.sent.tip"),
            set_title: lang.tr("settings.title"),
            grp_general: lang.tr("settings.group.general"),
            grp_printing: lang.tr("settings.group.printing"),
            lbl_language: lang.tr("settings.language"),
            lbl_printer: lang.tr("settings.printer"),
            def_printer: lang.tr("settings.printer.default"),
            lbl_duplex: lang.tr("settings.duplex"),
            lbl_color: lang.tr("settings.color"),
            lbl_paper: lang.tr("settings.paper"),
            paper_default: lang.tr("settings.paper.default"),
            paper_auto: lang.tr("settings.paper.auto"),
            legacy_note: lang.tr("settings.legacyNote"),
            close: lang.tr("settings.close"),
            gear_tip: lang.tr("settings.tip"),
            next_job_hint: lang.tr("settings.nextJob"),
            autostart: lang.tr("settings.autostart"),
            explorer_menu: lang.tr("settings.explorerMenu"),
            files_many_tpl: lang.tr("files.many"),
            print_n_tpl: lang.tr("btn.printN"),
            drop_add_tpl: lang.tr("drop.add"),
            prog_sent_tpl: lang.tr("progress.sent"),
            prog_errors_tpl: lang.tr("progress.errors"),
            prog_now_tpl: lang.tr("progress.now"),
            err_tip_tpl: lang.tr("status.error.tip"),
            def_printer_named_tpl: lang.tr("settings.printer.defaultNamed"),
            duplex_labels: [
                lang.tr("duplex.printer"),
                lang.tr("duplex.simplex"),
                lang.tr("duplex.long"),
                lang.tr("duplex.short"),
            ],
            color_labels: [
                lang.tr("color.printer"),
                lang.tr("color.color"),
                lang.tr("color.grayscale"),
            ],
        }
    }

    fn status(&self, key: &str) -> &str {
        match key {
            "status.waiting" => &self.st_waiting,
            "status.printing" => &self.st_printing,
            "status.sent" => &self.st_sent,
            _ => &self.st_error,
        }
    }
}

/// Progress bar drawn by hand: 26 px tall, accent fill, text centered both
/// ways — white over the fill, ink over the empty track.
fn paint_progress(p: &egui::Painter, rect: egui::Rect, frac: f32, text: &str) {
    let cr = CornerRadius::same(8);
    p.with_clip_rect(rect).rect_filled(rect, cr, Color32::from_rgb(226, 232, 240));
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
        p.with_clip_rect(rect).rect_filled(f, cr, theme::ACCENT);
    }
    let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(12.5), theme::INK);
    let pos = rect.center() - galley.size() / 2.0;
    // ink over the unfilled track, white over the accent fill
    let rest = egui::Rect::from_min_max(
        egui::pos2(rect.left() + rect.width() * frac, rect.top()),
        rect.max,
    );
    p.with_clip_rect(rest.intersect(rect)).galley(pos, galley.clone(), theme::INK);
    if let Some(f) = fill {
        p.with_clip_rect(f.intersect(rect))
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
    lang: Lang,
    /// (file name, display name) of every language in lang/
    lang_files: Vec<(String, String)>,
    /// rows that already played their appear-animation
    row_seen: std::collections::HashSet<PathBuf>,
    /// removed rows fading out
    ghosts: Vec<Ghost>,
    ghost_seq: u64,
    /// last known status of each row (to animate status changes)
    last_status: std::collections::HashMap<PathBuf, Status>,
    /// Explorer context-menu integration state (the registry is the truth)
    explorer_menu: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext, initial: Vec<PathBuf>) -> Self {
        install_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx);
        let cfg = Config::path()
            .as_deref()
            .map(Config::load)
            .unwrap_or_default();
        let ldir = lang::lang_dir();
        let lang_files = lang::available(ldir.as_deref());
        let lang = Lang::load(ldir.as_deref(), &cfg.language);
        let (default_printer, printers) = list_printers();
        let printer = if !cfg.printer.is_empty()
            && printers.iter().any(|p| *p == cfg.printer)
        {
            cfg.printer.clone()
        } else if printers.iter().any(|p| p == DEFAULT_PRINTER) {
            DEFAULT_PRINTER.to_string()
        } else {
            default_printer.clone().unwrap_or_default()
        };
        let queue: Queue = Arc::new(Mutex::new(Vec::new()));
        // hidden demo mode (ETP_DEMO=1) — UI preview with a fake queue
        if std::env::var("ETP_DEMO").is_ok() && initial.is_empty() {
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
        } else if !initial.is_empty() {
            *queue.lock().unwrap() = initial.iter().map(|p| Job::new(p.clone())).collect();
        }
        Self {
            queue,
            worker: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            printer,
            printers,
            default_printer,
            autostart: cfg.autostart,
            settings_open: std::env::var("ETP_DEMO").as_deref() == Ok("2"),
            drag: None,
            engine: print::EngineKind::detect(),
            pset: print::PrintSettings {
                duplex: print::DuplexMode::from_code(&cfg.duplex),
                color: print::ColorMode::from_code(&cfg.color),
                paper: cfg.paper,
            },
            lang,
            lang_files,
            row_seen: std::collections::HashSet::new(),
            ghosts: Vec::new(),
            ghost_seq: 0,
            last_status: std::collections::HashMap::new(),
            explorer_menu: shell::menu_registered(),
        }
    }

    fn printing(&self) -> bool {
        self.worker.is_some()
    }

    /// Persists the current settings next to the executable.
    fn save_cfg(&self) {
        let Some(path) = Config::path() else { return };
        Config {
            language: self.lang.file.clone(),
            printer: self.printer.clone(),
            autostart: self.autostart,
            duplex: self.pset.duplex.code().into(),
            color: self.pset.color.code().into(),
            paper: self.pset.paper.clone(),
        }
        .save(&path);
    }

    fn set_language(&mut self, file: &str) {
        self.lang = Lang::load(lang::lang_dir().as_deref(), file);
        self.save_cfg();
        // refresh the context-menu label to the new language
        if self.explorer_menu {
            if let Some(exe) = std::env::current_exe().ok() {
                self.explorer_menu = shell::register_menu(&self.lang.tr("explorer.add"), &exe);
            }
        }
    }

    /// Starts the fade-out animation of a removed row.
    fn push_ghost(&mut self, ctx: &egui::Context, name: String, status: Status) {
        self.ghost_seq += 1;
        let key = self.ghost_seq;
        // seed the animation at full visibility, then let it fade to 0
        ctx.animate_value_with_time(egui::Id::new(("ghost", key)), 1.0, 1e-9);
        self.ghosts.push(Ghost { name, status, key });
        if self.ghosts.len() > 12 {
            self.ghosts.remove(0);
        }
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
        let settings = self.pset.clone();
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

        // 1b. Files forwarded from other instances (Explorer context menu)
        let inbox_files = shell::take_inbox();
        let files_forwarded = !inbox_files.is_empty();
        if files_forwarded {
            let mut q = self.queue.lock().unwrap();
            for p in inbox_files {
                add_unique(&mut q, p);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // keep polling the inbox even when idle
        ctx.request_repaint_after(Duration::from_millis(300));

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

        let t = UiTexts::build(&self.lang);

        // window corners: true transparency isn't reliably supported on
        // Windows + OpenGL (the alpha renders as black), so the window
        // stays square — 0 also squares the close-button hover fill
        let corner: u8 = 0;

        // title bar flush to the top edges
        egui::TopBottomPanel::top("app-titlebar")
            .frame(
                egui::Frame::default()
                    .fill(theme::CARD)
                    .inner_margin(egui::Margin::same(0))
                    .corner_radius(CornerRadius {
                        nw: corner,
                        ne: corner,
                        sw: 0,
                        se: 0,
                    }),
            )
            .show(ctx, |ui| {
                title_bar(ui, "EasyToPrint — bulk PDF printing", corner);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(theme::PAPER)
                    .inner_margin(egui::Margin::same(22))
                    .corner_radius(CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: corner,
                        se: corner,
                    }),
            )
            .show(ctx, |ui| {
                // ---- Header: CMYK + title, counter and gear to the right ----
                ui.horizontal(|ui| {
                    cmyk_mark(ui, 8.0);
                    ui.add_space(4.0);
                    ui.label(RichText::new("EasyToPrint").size(21.0).color(theme::INK));
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        if gear_button(ui, &t.gear_tip).clicked() {
                            open_settings = true;
                        }
                        if total > 0 {
                            let chip = if total == 1 {
                                t.files_one.clone()
                            } else {
                                fill(&t.files_many_tpl, "n", &total.to_string())
                            };
                            badge(ui, &chip, theme::INK_SOFT, theme::GRAY_BG);
                        }
                    });
                });
                ui.add_space(10.0);

                // ---- Drop zone: a single accent card (the Print-button color) ----
                let dragging = !hovered_files.is_empty();
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 190.0),
                    Sense::click(),
                );
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                let hov_t = ui.ctx().animate_value_with_time(
                    egui::Id::new("drop_hover"),
                    if resp.hovered() { 1.0 } else { 0.0 },
                    0.15,
                );
                let sub = if dragging {
                    let names = hovered_files.join(", ");
                    let names: String = names.chars().take(60).collect();
                    fill(&t.drop_add_tpl, "names", &names)
                } else {
                    t.drop_sub.clone()
                };
                let p = ui.painter();
                let cr = CornerRadius::same(12);
                let drag_t = ui.ctx().animate_value_with_time(
                    egui::Id::new("drop_drag"),
                    if dragging { 1.0 } else { 0.0 },
                    0.12,
                );
                let hovered_col = mix(theme::ACCENT, Color32::from_rgb(17, 141, 172), hov_t);
                let zone_fill = mix(hovered_col, Color32::from_rgb(11, 100, 124), drag_t);
                p.rect_filled(rect, cr, zone_fill);
                if dragging {
                    p.rect_stroke(
                        rect,
                        cr,
                        Stroke::new(
                            2.0_f32,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 110),
                        ),
                        StrokeKind::Middle,
                    );
                    // process-color strip on the edge of the card
                    let top = rect.top() + 16.0;
                    let seg = (rect.height() - 32.0) / 4.0;
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
                }
                // compact cluster: arrow circle right above the texts
                let c = rect.center();
                let white = Color32::WHITE;
                let soft = Color32::from_rgba_unmultiplied(255, 255, 255, 205);
                p.circle_filled(
                    egui::pos2(c.x, c.y - 32.0),
                    16.0,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 46),
                );
                p.text(
                    egui::pos2(c.x, c.y - 31.0),
                    Align2::CENTER_CENTER,
                    "↓",
                    FontId::proportional(17.0),
                    white,
                );
                p.text(
                    egui::pos2(c.x, c.y + 9.0),
                    Align2::CENTER_CENTER,
                    if dragging { &t.drop_active } else { &t.drop_main },
                    FontId::proportional(16.0),
                    white,
                );
                p.text(
                    egui::pos2(c.x, c.y + 36.0),
                    Align2::CENTER_CENTER,
                    &sub,
                    FontId::proportional(12.0),
                    soft,
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
                                RichText::new(&t.btn_printing)
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
                                    RichText::new(&t.btn_stop)
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
                            fill(&t.print_n_tpl, "n", &waiting.to_string())
                        } else {
                            t.btn_print.clone()
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
                            egui::Button::new(
                                RichText::new(&t.btn_clear).size(13.5).color(theme::INK),
                            )
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
                        RichText::new(&t.warn_engine)
                            .size(12.0)
                            .color(theme::MAGENTA_TX),
                    );
                }

                // ---- Progress (collapses away when the queue is empty) ----
                let prog_h = ui.ctx().animate_value_with_time(
                    egui::Id::new("progress_h"),
                    if total > 0 { 26.0 } else { 0.0 },
                    0.28,
                );
                if prog_h > 0.5 {
                    ui.add_space(6.0 * (prog_h / 26.0).min(1.0));
                    let fin = done + errors;
                    let frac = fin as f32 / total as f32;
                    let mut text = fill(&t.prog_sent_tpl, "done", &fin.to_string());
                    text = fill(&text, "total", &total.to_string());
                    if errors > 0 {
                        text.push_str(&fill(&t.prog_errors_tpl, "n", &errors.to_string()));
                    }
                    // smooth progress: the bar eases toward the real value
                    let frac_anim = ui.ctx().animate_value_with_time(
                        egui::Id::new("progress_fill"),
                        frac,
                        0.45,
                    );
                    let (prect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), prog_h),
                        Sense::hover(),
                    );
                    paint_progress(ui.painter(), prect, frac_anim, &text);
                    // "Printing: …" line, fading in/out
                    let printing_now = snapshot.iter().any(|(_, _, s)| *s == Status::Printing);
                    let now_h = ui.ctx().animate_value_with_time(
                        egui::Id::new("now_h"),
                        if printing_now { 17.0 } else { 0.0 },
                        0.25,
                    );
                    if now_h > 0.5 {
                        let (nrect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), now_h),
                            Sense::hover(),
                        );
                        let alpha = (now_h / 17.0).min(1.0);
                        let line = match snapshot.iter().find(|(_, _, s)| *s == Status::Printing) {
                            Some((_, name, _)) => fill(&t.prog_now_tpl, "name", name),
                            None => String::new(),
                        };
                        ui.painter().text(
                            nrect.left_top(),
                            Align2::LEFT_CENTER,
                            line,
                            FontId::proportional(12.0),
                            faded(theme::AMBER_TX, alpha),
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
                            // empty state fades in (e.g. after "Clear")
                            let empty_t = ui.ctx().animate_value_with_time(
                                egui::Id::new("empty_state"),
                                1.0,
                                0.3,
                            );
                            let eh = 170.0 * empty_t.max(0.15);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), eh),
                                Sense::hover(),
                            );
                            let c = rect.center();
                            ui.painter().text(
                                c,
                                Align2::CENTER_CENTER,
                                &t.empty_title,
                                FontId::proportional(14.0),
                                faded(theme::INK_SOFT, empty_t),
                            );
                            ui.painter().text(
                                egui::pos2(c.x, c.y + 24.0),
                                Align2::CENTER_CENTER,
                                &t.empty_sub,
                                FontId::proportional(12.0),
                                faded(theme::INK_SOFT, empty_t),
                            );
                        } else {
                            // table header — fades/slides in with the first file
                            let head_t = ui.ctx().animate_value_with_time(
                                egui::Id::new("table_head"),
                                1.0,
                                0.25,
                            );
                            let head_h = HEAD_H * head_t.max(0.25);
                            let (hr, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), head_h),
                                Sense::hover(),
                            );
                            let p = ui.painter();
                            let hc = faded(theme::INK_SOFT, head_t);
                            p.text(
                                egui::pos2(hr.left() + 38.0, hr.top() + HEAD_H / 2.0),
                                Align2::RIGHT_CENTER,
                                "#",
                                FontId::proportional(11.0),
                                hc,
                            );
                            p.text(
                                egui::pos2(hr.left() + 54.0, hr.top() + HEAD_H / 2.0),
                                Align2::LEFT_CENTER,
                                &t.col_file,
                                FontId::proportional(11.0),
                                hc,
                            );
                            p.text(
                                egui::pos2(hr.right() - 58.0, hr.top() + HEAD_H / 2.0),
                                Align2::RIGHT_CENTER,
                                &t.col_status,
                                FontId::proportional(11.0),
                                hc,
                            );
                            p.line_segment(
                                [
                                    egui::pos2(hr.left() + 4.0, hr.bottom() - 1.0),
                                    egui::pos2(hr.right() - 4.0, hr.bottom() - 1.0),
                                ],
                                Stroke::new(
                                    1.0_f32,
                                    faded(Color32::from_rgb(216, 214, 208), head_t),
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
                                        // appear animation: slide in + fade in
                                        let row_id = egui::Id::new(("row_in", path));
                                        if self.row_seen.insert(path.clone()) {
                                            // seed the animation at 0 on first sight
                                            ui.ctx()
                                                .animate_value_with_time(row_id, 0.0, 1e-9);
                                        }
                                        let ta = ui
                                            .ctx()
                                            .animate_value_with_time(row_id, 1.0, 0.22)
                                            .clamp(0.0, 1.0);
                                        let slide = (1.0 - ta) * -10.0;
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
                                            ui.painter().rect_filled(
                                                cell,
                                                CornerRadius::same(6),
                                                faded(bg, ta),
                                            );
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
                                        // gentle pulse while the row is being printed
                                        let pulse = if *status == Status::Printing {
                                            0.72 + 0.28 * (ui.input(|i| i.time as f32) * 2.2).sin()
                                        } else {
                                            1.0
                                        };
                                        // one-shot flash when the status changes
                                        let flash_id = egui::Id::new(("statchange", path));
                                        let status_changed = match self
                                            .last_status
                                            .insert(path.clone(), status.clone())
                                        {
                                            Some(prev) => prev != *status,
                                            None => false,
                                        };
                                        if status_changed {
                                            ui.ctx().animate_value_with_time(flash_id, 1.0, 1e-9);
                                        }
                                        let flash = ui
                                            .ctx()
                                            .animate_value_with_time(flash_id, 0.0, 0.55);
                                        if flash > 0.02 {
                                            let (_, _, fbc) = status.badge();
                                            ui.painter().rect_filled(
                                                cell,
                                                CornerRadius::same(6),
                                                faded(fbc, flash * 0.6),
                                            );
                                        }
                                        // row number (+1 px optical: digits have no
                                        // descenders, the line box sits low)
                                        ui.painter().text(
                                            egui::pos2(cell.left() + 38.0 + slide, cy + 1.0),
                                            Align2::RIGHT_CENTER,
                                            format!("{}", i + 1),
                                            FontId::monospace(11.5),
                                            faded(theme::INK_SOFT, ta),
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
                                        ui.painter().galley_with_override_text_color(
                                            egui::pos2(
                                                name_rect.left() + slide,
                                                cy - gal.size().y / 2.0 + 1.5,
                                            ),
                                            gal,
                                            faded(theme::INK, ta),
                                        );
                                        ui.interact(
                                            name_rect,
                                            egui::Id::new(("name", path)),
                                            Sense::hover(),
                                        )
                                        .on_hover_text(path.display().to_string());
                                        // status badge
                                        let (key, tx, bc) = status.badge();
                                        let galley = ui
                                            .painter()
                                            .layout_no_wrap(
                                                t.status(key).to_owned(),
                                                FontId::proportional(11.0),
                                                tx,
                                            );
                                        let bs = galley.size() + egui::vec2(16.0, 7.0);
                                        let brect = egui::Rect::from_min_size(
                                            egui::pos2(
                                                cell.right() - 46.0 - bs.x + slide,
                                                cy - bs.y / 2.0,
                                            ),
                                            bs,
                                        );
                                        ui.painter().rect_filled(
                                            brect,
                                            CornerRadius::same((bs.y / 2.0) as u8),
                                            faded(bc, ta * pulse),
                                        );
                                        ui.painter().galley_with_override_text_color(
                                            brect.center() - galley.size() / 2.0,
                                            galley,
                                            faded(tx, ta * pulse),
                                        );
                                        if status.tooltip_key().is_some() {
                                            let tip = match status {
                                                Status::Error(msg) => {
                                                    fill(&t.err_tip_tpl, "msg", msg)
                                                }
                                                _ => t.st_sent_tip.clone(),
                                            };
                                            ui.interact(
                                                brect,
                                                egui::Id::new(("badge", path)),
                                                Sense::hover(),
                                            )
                                            .on_hover_text(tip);
                                        }
                                        // remove control: borderless red ×,
                                        // soft red circle + hand cursor on hover
                                        if *status != Status::Printing {
                                            let xr = egui::Rect::from_center_size(
                                                egui::pos2(cell.right() - 21.0 + slide, cy),
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
                                                faded(
                                                    if hov { theme::MAGENTA_TX } else { theme::INK_SOFT },
                                                    ta,
                                                ),
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
                                    // removed rows fading out below the list
                                    for gh in self.ghosts.clone() {
                                        let gt = ui
                                            .ctx()
                                            .animate_value_with_time(
                                                egui::Id::new(("ghost", gh.key)),
                                                0.0,
                                                0.3,
                                            )
                                            .clamp(0.0, 1.0);
                                        if gt <= 0.02 {
                                            continue;
                                        }
                                        let gh_h = ROW_H * (0.55 + 0.45 * gt);
                                        let (gcell, _) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), gh_h),
                                            Sense::hover(),
                                        );
                                        let p = ui.painter();
                                        let cy = gcell.top() + ROW_H / 2.0;
                                        let slide = (1.0 - gt) * 26.0;
                                        let gal = p.layout_no_wrap(
                                            gh.name.clone(),
                                            FontId::monospace(12.5),
                                            theme::INK,
                                        );
                                        if gal.size().x > gcell.width() - RIGHT_ZONE {
                                            continue;
                                        }
                                        p.galley_with_override_text_color(
                                            egui::pos2(gcell.left() + 54.0 + slide, cy - gal.size().y / 2.0 + 1.5),
                                            gal,
                                            faded(theme::INK, gt),
                                        );
                                        let (txt, tx, bc) = gh.status.badge();
                                        let ttx = match &gh.status {
                                            Status::Waiting => t.st_waiting.clone(),
                                            Status::Printing => t.st_printing.clone(),
                                            Status::Done => t.st_sent.clone(),
                                            Status::Error(_) => t.st_error.clone(),
                                        };
                                        let bgal = p.layout_no_wrap(ttx, FontId::proportional(11.0), tx);
                                        let bs = bgal.size() + egui::vec2(16.0, 7.0);
                                        let brect = egui::Rect::from_min_size(
                                            egui::pos2(gcell.right() - 46.0 - bs.x + slide, cy - bs.y / 2.0),
                                            bs,
                                        );
                                        p.rect_filled(brect, CornerRadius::same((bs.y / 2.0) as u8), faded(bc, gt));
                                        p.galley_with_override_text_color(
                                            brect.center() - bgal.size() / 2.0,
                                            bgal,
                                            faded(tx, gt),
                                        );
                                        let _ = txt;
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
            let mut h = 468.0;
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
                .inner_margin(egui::Margin::same(0))
                .corner_radius(CornerRadius::same(10))
                .stroke(Stroke::new(1.0_f32, theme::HAIRLINE))
                .fill(theme::CARD);
            egui::Window::new(&t.set_title)
                .id(egui::Id::new("settings-window"))
                .default_pos(pos)
                .fixed_size([390.0, h])
                .frame(win_frame)
                .title_bar(false)
                .resizable(false)
                .collapsible(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    // custom header, identical in style to the app title bar;
                    // the window stays pinned to its default position
                    let (hrect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 40.0),
                        Sense::hover(),
                    );
                    let p = ui.painter();
                    p.text(
                        egui::pos2(hrect.left() + 14.0, hrect.center().y),
                        Align2::LEFT_CENTER,
                        &t.set_title,
                        FontId::proportional(12.5),
                        theme::INK,
                    );
                    p.line_segment(
                        [
                            hrect.left_bottom(),
                            egui::pos2(hrect.right(), hrect.bottom()),
                        ],
                        Stroke::new(1.0_f32, theme::HAIRLINE),
                    );
                    // close button (same visuals as the window close button)
                    let cbr = egui::Rect::from_min_max(
                        egui::pos2(hrect.right() - 42.0, hrect.top()),
                        hrect.right_bottom(),
                    );
                    let cresp =
                        ui.interact(cbr, egui::Id::new("settings-close"), Sense::click());
                    let chov = cresp.hovered();
                    if chov {
                        // rounded to match the modal corner
                        p.rect_filled(
                            cbr,
                            CornerRadius {
                                nw: 0,
                                ne: 10,
                                sw: 0,
                                se: 0,
                            },
                            Color32::from_rgb(232, 17, 35),
                        );
                    }
                    let ccol = if chov { Color32::WHITE } else { theme::INK_SOFT };
                    let cc = cbr.center();
                    p.line_segment(
                        [egui::pos2(cc.x - 5.0, cc.y - 5.0), egui::pos2(cc.x + 5.0, cc.y + 5.0)],
                        Stroke::new(1.4_f32, ccol),
                    );
                    p.line_segment(
                        [egui::pos2(cc.x - 5.0, cc.y + 5.0), egui::pos2(cc.x + 5.0, cc.y - 5.0)],
                        Stroke::new(1.4_f32, ccol),
                    );
                    if cresp.clicked() {
                        close = true;
                    }

                    egui::Frame::default()
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(336.0);
                    // ---- General ----
                    settings_group(ui, &t.grp_general, |ui| {
                        ui.label(
                            RichText::new(&t.lbl_language)
                                .size(12.5)
                                .color(theme::INK_SOFT),
                        );
                        let current = self.lang.file.clone();
                        let current_name = self.lang.name.clone();
                        egui::ComboBox::from_id_salt("language")
                            .selected_text(
                                RichText::new(current_name).size(13.0).color(theme::INK),
                            )
                            .width(312.0)
                            .show_ui(ui, |ui| {
                                for (file, name) in self.lang_files.clone() {
                                    let is_sel = file == current;
                                    if ui
                                        .selectable_label(is_sel, &name)
                                        .clicked()
                                        && !is_sel
                                    {
                                        self.set_language(&file);
                                    }
                                }
                            });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(&t.lbl_printer)
                                .size(12.5)
                                .color(theme::INK_SOFT),
                        );
                        let selected = if self.printer.is_empty() {
                            match &self.default_printer {
                                Some(d) => fill(&t.def_printer_named_tpl, "name", d),
                                None => t.def_printer.clone(),
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
                                        Some(d) => fill(&t.def_printer_named_tpl, "name", d),
                                        None => t.def_printer.clone(),
                                    };
                                    if ui.selectable_value(
                                        &mut self.printer,
                                        String::new(),
                                        def_label,
                                    )
                                    .changed()
                                    {
                                        self.save_cfg();
                                    }
                                    for p in self.printers.clone() {
                                        if ui
                                            .selectable_value(&mut self.printer, p.clone(), p)
                                            .changed()
                                        {
                                            self.save_cfg();
                                        }
                                    }
                                });
                        });
                        if self.printing() {
                            ui.label(
                                RichText::new(&t.next_job_hint)
                                    .size(11.5)
                                    .color(theme::AMBER_TX),
                            );
                        }
                        ui.add_space(6.0);
                        if ui
                            .checkbox(
                                &mut self.autostart,
                                RichText::new(&t.autostart).size(13.0).color(theme::INK),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .changed()
                        {
                            self.save_cfg();
                        }
                        if ui
                            .checkbox(
                                &mut self.explorer_menu,
                                RichText::new(&t.explorer_menu)
                                    .size(13.0)
                                    .color(theme::INK),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .changed()
                        {
                            if self.explorer_menu {
                                if let Some(exe) = std::env::current_exe().ok() {
                                    self.explorer_menu =
                                        shell::register_menu(&self.lang.tr("explorer.add"), &exe);
                                }
                            } else {
                                shell::unregister_menu();
                            }
                        }
                    });
                    // ---- Printing ----
                    settings_group(ui, &t.grp_printing, |ui| {
                        ui.label(
                            RichText::new(&t.lbl_duplex)
                                .size(12.5)
                                .color(theme::INK_SOFT),
                        );
                        let modes = [
                            print::DuplexMode::Printer,
                            print::DuplexMode::Simplex,
                            print::DuplexMode::LongEdge,
                            print::DuplexMode::ShortEdge,
                        ];
                        let sel = modes
                            .iter()
                            .position(|m| *m == self.pset.duplex)
                            .unwrap_or(0);
                        egui::ComboBox::from_id_salt("duplex")
                            .selected_text(
                                RichText::new(&t.duplex_labels[sel])
                                    .size(13.0)
                                    .color(theme::INK),
                            )
                            .width(312.0)
                            .show_ui(ui, |ui| {
                                for (i, m) in modes.iter().enumerate() {
                                    if ui
                                        .selectable_value(
                                            &mut self.pset.duplex,
                                            *m,
                                            t.duplex_labels[i].clone(),
                                        )
                                        .changed()
                                    {
                                        self.save_cfg();
                                    }
                                }
                            });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(&t.lbl_color)
                                .size(12.5)
                                .color(theme::INK_SOFT),
                        );
                        let modes = [
                            print::ColorMode::Printer,
                            print::ColorMode::Color,
                            print::ColorMode::Grayscale,
                        ];
                        let sel = modes
                            .iter()
                            .position(|m| *m == self.pset.color)
                            .unwrap_or(0);
                        egui::ComboBox::from_id_salt("color")
                            .selected_text(
                                RichText::new(&t.color_labels[sel])
                                    .size(13.0)
                                    .color(theme::INK),
                            )
                            .width(312.0)
                            .show_ui(ui, |ui| {
                                for (i, m) in modes.iter().enumerate() {
                                    if ui
                                        .selectable_value(
                                            &mut self.pset.color,
                                            *m,
                                            t.color_labels[i].clone(),
                                        )
                                        .changed()
                                    {
                                        self.save_cfg();
                                    }
                                }
                            });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(&t.lbl_paper)
                                .size(12.5)
                                .color(theme::INK_SOFT),
                        );
                        let selected = match self.pset.paper.as_str() {
                            "" => t.paper_default.clone(),
                            "auto" => t.paper_auto.clone(),
                            code => code.to_string(),
                        };
                        egui::ComboBox::from_id_salt("paper")
                            .selected_text(
                                RichText::new(selected).size(13.0).color(theme::INK),
                            )
                            .width(312.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut self.pset.paper,
                                        String::new(),
                                        t.paper_default.clone(),
                                    )
                                    .changed()
                                {
                                    self.save_cfg();
                                }
                                if ui
                                    .selectable_value(
                                        &mut self.pset.paper,
                                        print::PAPER_AUTO.to_string(),
                                        t.paper_auto.clone(),
                                    )
                                    .changed()
                                {
                                    self.save_cfg();
                                }
                                for p in print::PAPERS {
                                    if ui
                                        .selectable_value(
                                            &mut self.pset.paper,
                                            p.code.to_string(),
                                            p.code,
                                        )
                                        .changed()
                                    {
                                        self.save_cfg();
                                    }
                                }
                            });
                    });
                    // ---- footer ----
                    ui.add_space(6.0);
                    if matches!(
                        self.engine,
                        Some(print::EngineKind::PdfToPrinter(_))
                    ) {
                        ui.label(
                            RichText::new(&t.legacy_note)
                                .size(11.5)
                                .color(theme::AMBER_TX),
                        );
                    }
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(&t.close).size(13.0).color(theme::INK),
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
                });
            self.settings_open = open && !close;
        }

        // 5. Actions after drawing (no lock held during UI)
        if action_clear {
            let removed: Vec<(String, Status)> = {
                let mut g = self.queue.lock().unwrap();
                let r: Vec<_> = g
                    .iter()
                    .filter(|j| j.status != Status::Printing)
                    .map(|j| (j.name.clone(), j.status.clone()))
                    .collect();
                g.retain(|j| j.status == Status::Printing);
                r
            };
            for (name, status) in removed {
                self.push_ghost(ctx, name, status);
            }
        }
        if let Some(p) = remove_path {
            let removed = {
                let mut g = self.queue.lock().unwrap();
                let job = g
                    .iter()
                    .find(|j| j.path == p && j.status != Status::Printing)
                    .map(|j| (j.name.clone(), j.status.clone()));
                g.retain(|j| j.path != p || j.status == Status::Printing);
                job
            };
            if let Some((name, status)) = removed {
                self.push_ghost(ctx, name, status);
            }
        }
        if total == 0 {
            self.last_status.clear();
        }
        // drop finished fade-out ghosts
        self.ghosts.retain(|gh| {
            ctx.animate_value_with_time(egui::Id::new(("ghost", gh.key)), 0.0, 0.3) > 0.02
        });
        if action_start
            || ((files_added || files_picked || files_forwarded) && self.autostart)
        {
            self.start_worker();
        }
    }
}

fn main() -> eframe::Result<()> {
    // files passed on the command line (e.g. from the Explorer context menu)
    let initial: Vec<PathBuf> = std::env::args()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| p.is_file() && is_pdf(p))
        .collect();
    // a second instance forwards its files to the running one and exits
    if !shell::is_primary() {
        if !initial.is_empty() {
            shell::submit_paths(&initial);
        }
        return Ok(());
    }

    let icon = load_icon();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("EasyToPrint — bulk PDF printing")
        .with_decorations(false)
        .with_inner_size([620.0, 860.0])
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
        Box::new(|cc| Ok(Box::new(App::new(cc, initial)))),
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
