//! Printing engine.
//!
//! Primary mode: PDF pages are rendered with PDFium (pdfium.dll) and sent
//! straight to the printer through GDI. No external applications required.
//!
//! Fallback mode (when pdfium.dll is missing): the legacy external tool
//! PDFtoPrinter.exe, if present next to the program.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::*;

/// The available printing engine.
#[derive(Clone, Debug)]
pub enum EngineKind {
    /// Built-in: PDFium + GDI.
    Builtin,
    /// Fallback: external PDFtoPrinter.exe tool.
    PdfToPrinter(PathBuf),
}

fn exe_ancestors() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..4 {
            match cur {
                Some(d) => {
                    dirs.push(d.clone());
                    cur = d.parent().map(|p| p.to_path_buf());
                }
                None => break,
            }
        }
    }
    dirs
}

impl EngineKind {
    /// pdfium.dll next to the program takes priority; without it — the
    /// legacy external tool.
    pub fn detect() -> Option<EngineKind> {
        for dir in exe_ancestors() {
            if dir.join("pdfium.dll").is_file() {
                return Some(EngineKind::Builtin);
            }
        }
        for dir in exe_ancestors() {
            let cand = dir.join("PDFtoPrinter.exe");
            if cand.is_file() {
                return Some(EngineKind::PdfToPrinter(cand));
            }
        }
        None
    }

    pub fn label(&self) -> &'static str {
        match self {
            EngineKind::Builtin => "built-in (PDFium)",
            EngineKind::PdfToPrinter(_) => "external (legacy)",
        }
    }
}

// ---------------------------------------------------------------------------
// Print options (DEVMODE overrides of the printer's Windows defaults)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DuplexMode {
    /// the printer's own default
    #[default]
    Printer,
    Simplex,
    /// two-sided, flip on the long edge (books)
    LongEdge,
    /// two-sided, flip on the short edge (calendars)
    ShortEdge,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorMode {
    #[default]
    Printer,
    Color,
    Grayscale,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PaperMode {
    #[default]
    Printer,
    A4,
    Letter,
}

/// Overrides applied to every print job. `Printer` variants mean
/// "leave whatever Windows has configured as the printer default".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PrintSettings {
    pub duplex: DuplexMode,
    pub color: ColorMode,
    pub paper: PaperMode,
}

impl DuplexMode {
    pub fn label(&self) -> &'static str {
        match self {
            DuplexMode::Printer => "Printer default",
            DuplexMode::Simplex => "One-sided",
            DuplexMode::LongEdge => "Two-sided (long edge)",
            DuplexMode::ShortEdge => "Two-sided (short edge)",
        }
    }
}

impl ColorMode {
    pub fn label(&self) -> &'static str {
        match self {
            ColorMode::Printer => "Printer default",
            ColorMode::Color => "Color",
            ColorMode::Grayscale => "Grayscale",
        }
    }
}

impl PaperMode {
    pub fn label(&self) -> &'static str {
        match self {
            PaperMode::Printer => "Printer default",
            PaperMode::A4 => "A4",
            PaperMode::Letter => "Letter",
        }
    }
}

/// Writes the chosen overrides into a DEVMODE struct (pure — unit-testable).
#[cfg(windows)]
fn apply_settings(dm: &mut windows_sys::Win32::Graphics::Gdi::DEVMODEW, s: &PrintSettings) {
    use windows_sys::Win32::Graphics::Gdi::{
        DMCOLOR_COLOR, DMCOLOR_MONOCHROME, DMDUP_HORIZONTAL, DMDUP_SIMPLEX, DMDUP_VERTICAL,
        DMPAPER_A4, DMPAPER_LETTER, DM_COLOR, DM_DUPLEX, DM_PAPERSIZE,
    };
    match s.duplex {
        DuplexMode::Printer => {}
        DuplexMode::Simplex => {
            dm.dmDuplex = DMDUP_SIMPLEX;
            dm.dmFields |= DM_DUPLEX;
        }
        DuplexMode::LongEdge => {
            dm.dmDuplex = DMDUP_VERTICAL;
            dm.dmFields |= DM_DUPLEX;
        }
        DuplexMode::ShortEdge => {
            dm.dmDuplex = DMDUP_HORIZONTAL;
            dm.dmFields |= DM_DUPLEX;
        }
    }
    match s.color {
        ColorMode::Printer => {}
        ColorMode::Color => {
            dm.dmColor = DMCOLOR_COLOR;
            dm.dmFields |= DM_COLOR;
        }
        ColorMode::Grayscale => {
            dm.dmColor = DMCOLOR_MONOCHROME;
            dm.dmFields |= DM_COLOR;
        }
    }
    match s.paper {
        PaperMode::Printer => {}
        PaperMode::A4 => {
            dm.Anonymous1.Anonymous1.dmPaperSize = DMPAPER_A4 as i16;
            dm.dmFields |= DM_PAPERSIZE;
        }
        PaperMode::Letter => {
            dm.Anonymous1.Anonymous1.dmPaperSize = DMPAPER_LETTER as i16;
            dm.dmFields |= DM_PAPERSIZE;
        }
    }
}

/// Fetches the printer's default DEVMODE (with driver-private data),
/// applies the overrides and validates the result with the driver.
#[cfg(windows)]
fn devmode_buffer(printer: &str, settings: &PrintSettings) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{DEVMODEW, DM_IN_BUFFER, DM_OUT_BUFFER};
    use windows_sys::Win32::Graphics::Printing::{
        ClosePrinter, DocumentPropertiesW, OpenPrinterW, PRINTER_HANDLE,
    };
    unsafe {
        let name: Vec<u16> = printer.encode_utf16().chain(std::iter::once(0)).collect();
        let mut handle: PRINTER_HANDLE = PRINTER_HANDLE::default();
        if OpenPrinterW(name.as_ptr(), &mut handle, std::ptr::null()) == 0 {
            return None;
        }
        let size = DocumentPropertiesW(
            std::ptr::null_mut(),
            handle,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
        );
        if size <= 0 {
            ClosePrinter(handle);
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let rc = DocumentPropertiesW(
            std::ptr::null_mut(),
            handle,
            name.as_ptr(),
            buf.as_mut_ptr() as *mut DEVMODEW,
            std::ptr::null(),
            DM_OUT_BUFFER,
        );
        if rc < 0 {
            ClosePrinter(handle);
            return None;
        }
        {
            let dm = &mut *(buf.as_mut_ptr() as *mut DEVMODEW);
            apply_settings(dm, settings);
        }
        // let the driver validate (and possibly adjust) the modified settings
        let ok = DocumentPropertiesW(
            std::ptr::null_mut(),
            handle,
            name.as_ptr(),
            buf.as_mut_ptr() as *mut DEVMODEW,
            buf.as_ptr() as *const DEVMODEW,
            DM_IN_BUFFER | DM_OUT_BUFFER,
        );
        ClosePrinter(handle);
        if ok < 0 {
            return None;
        }
        Some(buf)
    }
}

/// PDFium must be initialized exactly once per process — a shared instance
/// is created on first use and reused for every subsequent print job.
static PDFIUM: OnceLock<Option<Pdfium>> = OnceLock::new();

fn shared_pdfium() -> Option<&'static Pdfium> {
    PDFIUM
        .get_or_init(|| {
            Pdfium::bind_to_system_library()
                .ok()
                .map(Pdfium::new)
        })
        .as_ref()
}

/// Prints a single PDF file with the selected engine.
/// Blocks until the job has been handed over to the print spooler.
pub fn print_pdf(
    engine: &EngineKind,
    pdf: &Path,
    printer: Option<&str>,
    settings: &PrintSettings,
) -> Result<(), String> {
    match engine {
        EngineKind::Builtin => {
            #[cfg(windows)]
            {
                let printer = match printer.map(str::trim).filter(|s| !s.is_empty()) {
                    Some(p) => p.to_string(),
                    None => default_printer_name()
                        .ok_or("failed to read the default printer")?,
                };
                let Some(pdfium) = shared_pdfium() else {
                    return Err("failed to load pdfium.dll".into());
                };
                let dm = devmode_buffer(&printer, settings);
                gdi_print(pdfium, pdf, &printer, dm.as_deref())
            }
            #[cfg(not(windows))]
            Err("built-in printing is supported on Windows only".into())
        }
        EngineKind::PdfToPrinter(tool) => {
            let mut cmd = std::process::Command::new(tool);
            cmd.arg(pdf);
            if let Some(p) = printer.map(str::trim).filter(|s| !s.is_empty()) {
                cmd.arg(p);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd.output() {
                Ok(out) if out.status.success() => Ok(()),
                Ok(out) => Err(format!(
                    "external tool exited with code {}",
                    out.status.code().unwrap_or(-1)
                )),
                Err(e) => Err(format!("failed to launch the tool ({e})")),
            }
        }
    }
}

#[cfg(windows)]
fn default_printer_name() -> Option<String> {
    use windows_sys::Win32::Graphics::Printing::GetDefaultPrinterW;
    unsafe {
        let mut len: u32 = 0;
        GetDefaultPrinterW(std::ptr::null_mut(), &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]))
    }
}

/// Sends a PDF through GDI: each page is rendered to a bitmap (PDFium)
/// and drawn onto the printer device context with StretchDIBits.
#[cfg(windows)]
pub(crate) fn gdi_print(
    pdfium: &Pdfium,
    pdf: &Path,
    printer: &str,
    devmode: Option<&[u8]>,
) -> Result<(), String> {
    use windows_sys::Win32::Graphics::Gdi as g;
    use windows_sys::Win32::Storage::Xps as xps;

    let doc = pdfium
        .load_pdf_from_file(pdf, None)
        .map_err(|e| format!("failed to open the file ({e})"))?;

    let dev: Vec<u16> = printer.encode_utf16().chain(std::iter::once(0)).collect();
    let doc_name: Vec<u16> = "EasyToPrint"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let dm_ptr = devmode
            .map(|b| b.as_ptr() as *const g::DEVMODEW)
            .unwrap_or(std::ptr::null());
        let hdc = g::CreateDCW(std::ptr::null(), dev.as_ptr(), std::ptr::null(), dm_ptr);
        if hdc.is_null() {
            return Err(format!("failed to open printer “{printer}”"));
        }
        let di = xps::DOCINFOW {
            cbSize: std::mem::size_of::<xps::DOCINFOW>() as i32,
            lpszDocName: doc_name.as_ptr(),
            lpszOutput: std::ptr::null(),
            lpszDatatype: std::ptr::null(),
            fwType: 0,
        };
        if xps::StartDocW(hdc, &di) <= 0 {
            g::DeleteDC(hdc);
            return Err("failed to start the print job".into());
        }

        let mut result = Ok(());
        for page in doc.pages().iter() {
            if xps::StartPage(hdc) <= 0 {
                result = Err("failed to start a page".into());
                break;
            }
            let res = render_page(hdc, &page);
            xps::EndPage(hdc);
            if let Err(e) = res {
                result = Err(e);
                break;
            }
        }

        let end_ok = xps::EndDoc(hdc) > 0;
        g::DeleteDC(hdc);
        if !end_ok && result.is_ok() {
            result = Err("failed to finish the print job".into());
        }
        result
    }
}

/// Renders one page into a bitmap at the printer's DPI and draws it,
/// aspect-ratio preserved and centered, inside the printable area.
#[cfg(windows)]
unsafe fn render_page(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    page: &PdfPage,
) -> Result<(), String> {
    use windows_sys::Win32::Graphics::Gdi as g;

    let w_pts = page.width().value as f64;
    let h_pts = page.height().value as f64;
    let dpi = g::GetDeviceCaps(hdc, g::LOGPIXELSX as i32) as f64;
    if dpi <= 0.0 {
        return Err("the printer did not report its DPI".into());
    }
    let px_w = (w_pts * dpi / 72.0).round().max(1.0) as i32;
    let px_h = (h_pts * dpi / 72.0).round().max(1.0) as i32;

    let config = PdfRenderConfig::new()
        .set_format(PdfBitmapFormat::BGRx)
        // native BGRx byte layout — exactly what GDI expects (BI_RGB, 32bpp)
        .set_reverse_byte_order(false)
        .set_fixed_size(px_w, px_h)
        .set_clear_color(PdfColor::WHITE);
    let bmp = page
        .render_with_config(&config)
        .map_err(|e| format!("page rendering failed ({e})"))?;
    if bmp.width() != px_w || bmp.height() != px_h {
        return Err("unexpected rendered page size".into());
    }
    let bytes = bmp.as_raw_bytes();

    let bmi = g::BITMAPINFO {
        bmiHeader: g::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<g::BITMAPINFOHEADER>() as u32,
            biWidth: px_w,
            biHeight: -px_h, // top-down row order
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            biSizeImage: bytes.len() as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [g::RGBQUAD::default()],
    };

    let horz = g::GetDeviceCaps(hdc, g::HORZRES as i32) as f64;
    let vert = g::GetDeviceCaps(hdc, g::VERTRES as i32) as f64;
    if horz <= 0.0 || vert <= 0.0 {
        return Err("the printer did not report a printable area".into());
    }
    let sf = (horz / px_w as f64).min(vert / px_h as f64);
    let dw = (px_w as f64 * sf).round() as i32;
    let dh = (px_h as f64 * sf).round() as i32;
    let dx = ((horz - dw as f64) / 2.0).round() as i32;
    let dy = ((vert - dh as f64) / 2.0).round() as i32;

    let copied = g::StretchDIBits(
        hdc, dx, dy, dw, dh, 0, 0, px_w, px_h,
        bytes.as_ptr() as *const core::ffi::c_void,
        &bmi,
        g::DIB_RGB_COLORS,
        g::SRCCOPY,
    );
    if copied < 0 {
        return Err("failed to send the page to the printer".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (run only with ETP_PRINTTEST=1 — print through a file-port printer)
// ---------------------------------------------------------------------------

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn sample_pdf() -> Vec<u8> {
        let stream = b"BT /F1 24 Tf 72 700 Td (EasyToPrint test) Tj ET";
        format!(
            "%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
             2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
             3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Resources<</Font<</F1 4 0 R>>>>/Contents 5 0 R>>endobj\n\
             4 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n\
             5 0 obj<</Length {}>>stream\n{}\nendstream endobj\n\
             trailer<</Size 6/Root 1 0 R>>\n",
            stream.len(),
            String::from_utf8_lossy(stream)
        )
        .into_bytes()
    }

    #[test]
    fn print_two_files_in_a_row() {
        if std::env::var("ETP_PRINTTEST").is_err() {
            eprintln!("skipping the print test (ETP_PRINTTEST is not set)");
            return;
        }
        let printer =
            std::env::var("ETP_TEST_PRINTER").unwrap_or_else(|_| "ETP Test PDF".into());
        let dir = std::env::temp_dir();
        let src = dir.join("etp_test_src.pdf");
        let out = std::path::PathBuf::from("C:/repos/easytoprint/etp_test_out.pdf");
        let _ = std::fs::remove_file(&out);
        std::fs::write(&src, sample_pdf()).unwrap();

        let engine = EngineKind::Builtin;
        let opts = PrintSettings::default();
        // two jobs in a row — must not hit "library is already initialized"
        print_pdf(&engine, &src, Some(&printer), &opts).expect("first print failed");
        print_pdf(&engine, &src, Some(&printer), &opts).expect("second print failed");

        // the printout is asynchronous — wait for the spooler to write the file
        let mut len = 0u64;
        for _ in 0..60 {
            if let Ok(meta) = std::fs::metadata(&out) {
                if meta.len() > 1_000 {
                    len = meta.len();
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(len > 1_000, "output PDF was not created (size: {len} B)");

        // the text sits in the upper half of the page — if the image were
        // flipped, dark pixels would end up at the bottom
        let Some(pdfium) = shared_pdfium() else { panic!("pdfium unavailable") };
        let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
        let page = doc.pages().iter().next().expect("no page in the output");
        let bmp = page
            .render_with_config(&PdfRenderConfig::new().scale_page_by_factor(0.5))
            .unwrap();
        let rgba = bmp.as_rgba_bytes();
        let w = bmp.width() as usize;
        let h = bmp.height() as usize;
        let stride = rgba.len() / h;
        let dark_in_rows = |from: usize, to: usize| -> usize {
            rgba[from * stride..to * stride]
                .chunks(stride)
                .map(|row| {
                    row.chunks_exact(4)
                        .take(w)
                        .filter(|px| px[0] < 128 && px[1] < 128 && px[2] < 128)
                        .count()
                })
                .sum()
        };
        let top = dark_in_rows(0, h / 2);
        let bottom = dark_in_rows(h / 2, h);
        assert!(
            top > bottom,
            "the printout appears flipped (dark pixels: top={top}, bottom={bottom})"
        );
    }

    #[test]
    fn devmode_overrides_are_written() {
        if cfg!(not(windows)) {
            return;
        }
        use windows_sys::Win32::Graphics::Gdi::{
            DMCOLOR_MONOCHROME, DMDUP_VERTICAL, DMPAPER_A4, DM_COLOR, DM_DUPLEX, DM_PAPERSIZE,
            DEVMODEW,
        };
        let mut dm = DEVMODEW::default();
        let s = PrintSettings {
            duplex: DuplexMode::LongEdge,
            color: ColorMode::Grayscale,
            paper: PaperMode::A4,
        };
        apply_settings(&mut dm, &s);
        assert_eq!(dm.dmDuplex, DMDUP_VERTICAL);
        assert_eq!(dm.dmColor, DMCOLOR_MONOCHROME);
        assert_eq!(unsafe { dm.Anonymous1.Anonymous1.dmPaperSize }, DMPAPER_A4 as i16);
        assert_eq!(dm.dmFields & DM_DUPLEX, DM_DUPLEX);
        assert_eq!(dm.dmFields & DM_COLOR, DM_COLOR);
        assert_eq!(dm.dmFields & DM_PAPERSIZE, DM_PAPERSIZE);
        // "Printer default" must leave the DEVMODE untouched
        let mut dm = DEVMODEW::default();
        apply_settings(&mut dm, &PrintSettings::default());
        assert_eq!(dm.dmFields, 0);
        assert_eq!(dm.dmDuplex, 0);
    }
}
