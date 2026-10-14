@echo off
setlocal
:: Build EasyToPrint
:: Requirements: rustup (stable-x86_64-pc-windows-gnu toolchain) + MSYS2 (mingw-w64-x86_64-gcc)
set PATH=C:\msys64\mingw64\bin;%USERPROFILE%\.cargo\bin;%PATH%

:: pdfium.dll - the PDF rendering engine (downloaded once)
if not exist pdfium.dll (
  echo Downloading pdfium.dll...
  powershell -NoProfile -Command "Invoke-WebRequest -Uri 'https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-win-x64.tgz' -OutFile pdfium.tgz"
  tar -xzf pdfium.tgz bin/pdfium.dll
  move /y bin\pdfium.dll pdfium.dll >nul
  rd /s /q bin 2>nul
  del pdfium.tgz 2>nul
)

cargo build --release
if errorlevel 1 exit /b 1

mkdir dist 2>nul
copy /y target\release\easytoprint.exe dist\ >nul
copy /y pdfium.dll dist\ >nul
echo Done: dist\easytoprint.exe + dist\pdfium.dll
