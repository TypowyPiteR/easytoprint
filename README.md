# EasyToPrint

A lightweight Windows application for **bulk printing PDF files** — simple,
light-themed interface with drag & drop support.

Drop dozens of documents at once (invoices, contracts, reports) and the app
queues them and sends them to the printer **one by one**, showing the live
status of every file. Each PDF arrives as a separate print job.

## Features

- **drag & drop** of many PDF files at once — add more at any time, even mid-print
- **per-file status** as a colored badge in the queue table:
  *Waiting / Printing… / Sent / Error* (details on hover)
- **progress bar** and error counter
- **printer selection** from the system list
- **Stop** — finishes the current file, never cuts it in half
- **Clear** — empties the queue, **×** — removes single files
- optional **"Print immediately after adding files"** mode
- **multiple languages** — plain JSON files in `lang/`; drop in a new
  `xx.json` to add a language (English and Polish are included);
  the chosen language and all settings are remembered
- paper-size override: ISO A0–A7, B0–B6, C4–C6, DL and Letter
- duplex and color overrides (printer default / one-sided / two-sided,
  color / grayscale)
- light "paper & CMYK" visual theme, filenames set in Consolas

## Download

Grab the latest build from the **[Releases](https://github.com/TypowyPiteR/easytoprint/releases)**
page, unzip, and keep the layout as-is:

```
easytoprint.exe   <- the program
lib/pdfium.dll    <- the PDF rendering library (required)
lang/             <- interface languages (en, pl, …)
```

## How it works

Pages are rendered by the program itself using **PDFium** (the same engine
that powers Google Chrome) and sent straight to the selected printer through
the Windows GDI interface, at the printer's native resolution, aspect ratio
preserved. No external applications are required.

The print worker takes files from the queue one at a time, pausing briefly
between jobs so the printer spooler can keep up.

## Building from source

Requirements: [Rust](https://rustup.rs) and [MSYS2](https://www.msys2.org)
with the `mingw-w64-x86_64-gcc` package.

```
build.bat     // downloads pdfium.dll (once), builds, prepares dist\
```

Tests (including a paperless print test through a file-port printer):

```
set ETP_PRINTTEST=1
cargo test --release
```

UI preview without printing: run with `ETP_DEMO=1` (fake queue);
`ETP_DEMO=2` additionally opens the settings window.

## Tech

- GUI: [egui](https://github.com/emilk/egui) (Rust, immediate mode)
- PDF rendering: [PDFium](https://github.com/google/pdfium) via [pdfium-render](https://crates.io/crates/pdfium-render)
- Printing: Windows GDI (`StartDoc` / `StretchDIBits`); printer list: `EnumPrintersW`

Layout: `src/main.rs` (GUI + queue) and `src/print.rs` (print engine).

## Licenses

- Application code: **MIT** — see [LICENSE](LICENSE)
- pdfium.dll and its components: permissive licenses (BSD and friends) —
  full texts in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)
