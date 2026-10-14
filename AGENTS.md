# AGENTS.md

Working notes for AI coding assistants on this repository. Read before touching anything.

## What this is

EasyToPrint — a lightweight Windows app (Rust + egui) for bulk-printing PDF files.
Drag & drop a queue, files are printed one by one by a built-in engine
(PDFium rendering → Windows GDI). No external applications needed.

## Communication rules (from the owner, Piotr / TypowyPiteR)

- **Talk to the owner in Polish.** He is not a professional developer — explain
  concepts simply, avoid unexplained jargon.
- **All code, comments, commit messages, UI texts, docs: English.**
- **Never create a GitHub Release, tag or zip on your own** — releases happen
  only on an explicit request ("zrób release"). Commits and pushes after every
  change are expected and welcome.
- After `git push`, always show the full output (don't pipe through `tail` —
  a silently failed push happened once and cost us a debugging session).
- Versioning: pre-1.0 (`0.x.y`). Feature → bump `x`, fix → bump `y`.
  1.0.0 only when the app is stable in daily use.
- GUI feedback loop that works with this owner: he gives short, precise lists
  of visual complaints. Verify every GUI change with a screenshot +
  analysis; for pixel-level questions measure the screenshot programmatically
  (PowerShell + System.Drawing) instead of eyeballing.

## Build & toolchain (Windows, GNU target)

```
set PATH=C:\msys64\mingw64\bin;%USERPROFILE%\.cargo\bin;%PATH%
build.bat          # downloads pdfium.dll once, builds, refreshes dist\
```

- Toolchain: `stable-x86_64-pc-windows-gnu` + MSYS2 `mingw-w64-x86_64-gcc`
  (provides dlltool/as — required by the `windows-*` crates; the rustup
  self-contained dlltool is NOT enough).
- `build.rs` embeds `assets/icon.ico` into the exe via `windres`
  (in MSYS2 it is plain `windres.exe`, NOT `x86_64-w64-mingw32-windres`).
- Installer: Inno Setup 6, per-user install at
  `%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe`;
  script: `installer/easytoprint.iss` (bump `MyAppVersion` when releasing).

## Layout

- `src/main.rs` — GUI (egui) + queue logic. UI strings go through `UiTexts`
  built from `Lang`; never hardcode visible text.
- `src/print.rs` — print engine: PDFium + GDI, DEVMODE overrides
  (duplex/color/paper), per-page paper switching for "auto" mode.
- `src/lang.rs` — translations: `lang/*.json` overlay on an English base
  **embedded in the binary** (`include_str!` of `lang/en.json`).
  Missing keys always fall back to English — tests enforce this.
- `src/config.rs` — persisted settings (`easytoprint.json`): next to the exe
  when the folder is writable (portable), else `%APPDATA%\EasyToPrint`.
- `lang/` — one JSON per language (24 official EU languages ship).
  Adding a language = dropping a new `xx.json`; nothing else.
- `lib/pdfium.dll` — dev copy; runtime layout is `easytoprint.exe` +
  `lib/pdfium.dll` + `lang/` (searched up to 4 parent folders of the exe).
- `dist/` — build output (gitignored except README.txt).

## Hard-won gotchas

- **PDFium must be initialized exactly once per process** (`shared_pdfium()`
  with a `OnceLock`). Creating a second `Pdfium` fails with
  "library is already initialized" — there is a regression test for this.
- egui 0.33 API notes: `allocate_rect`/`allocate_exact_size` return
  `Response` / `(Rect, Response)` respectively (not uniform!);
  `Margin` takes `i8`; `rect_stroke` needs a `StrokeKind`; text is best
  drawn through the painter for pixel-perfect vertical centering
  (line-box vs glyph centering — see the +1.5 px optical nudge on filenames).
- Queue rows are allocated by egui (`allocate_exact_size` + controlled
  `item_spacing`); the returned rect is the single source of truth.
  Don't hand-build row rectangles — that architecture was tried and reverted.
- StartDoc/StartPage/EndDoc/DOCINFOW live in `Win32::Storage::Xps`,
  not Gdi. `PRINTER_HANDLE` is a struct, use `PRINTER_HANDLE::default()`.
- Windows printer settings flow through DEVMODE: get defaults via
  `DocumentPropertiesW(DM_OUT_BUFFER)`, mutate, validate with
  `DM_IN_BUFFER|DM_OUT_BUFFER`, pass to `CreateDCW`. Paper sizes without a
  DMPAPER constant are sent as dmPaperLength/Width (tenths of mm).

## Testing

```
cargo test --release          # 8 unit tests, no printer needed
set ETP_PRINTTEST=1           # + real (paperless) print test:
cargo test --release          # needs a file-port printer "ETP Test PDF"
```

The print test creates a printer with the "Microsoft Print To PDF" driver and
a file port (see `print.rs` tests), verifies output content and orientation,
then removes it. Env helpers: `ETP_DEMO=1` (fake queue), `ETP_DEMO=2`
(+ settings window), `ETP_DEBUG=1` (row geometry guide lines).

## Git / release policy

- Binaries never go into git (`.gitignore`: target/, pdfium.dll, dist exes…).
- Commits: English, imperative, one logical change. Push after every change
  with visible output.
- Release (only on request): bump `Cargo.toml` version + `MyAppVersion` in
  the .iss, `build.bat`, compile installer, make the portable zip
  (`easytoprint.exe + lib + lang + README.txt`), `gh release create vX.Y.Z`
  with **both** assets and SHA-256 checksums, English notes.
