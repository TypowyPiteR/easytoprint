EasyToPrint — bulk PDF printing
===============================

Getting started
---------------
1) Make sure the layout is in place:
   - easytoprint.exe
   - lib\pdfium.dll        (the PDF rendering library)
   - lang\                 (interface languages)

2) Run easytoprint.exe.

3) Drag & drop PDF files onto the light drop zone
   (or click the zone to browse for files).
   You can keep adding files while printing is in progress.
   Tip: enable "Add to EasyToPrint" in Settings to get a right-click
   menu entry in Windows Explorer — select many PDFs, right-click, add.

4) Click "Print" — files are sent to the printer ONE BY ONE,
   with a short pause between jobs (for the printer spooler).

File statuses
-------------
Waiting             - the file is queued
Printing…           - the file is being sent to the printer right now
Sent                - done (badge tooltip: "Sent to the printer")
Error               - something went wrong (details on badge hover)

Settings (gear button, top-right corner)
----------------------------------------
Language                     - interface language (add more via lang\*.json)
Printer                      - pick a printer from the system list
                               ("Default" = the Windows default printer)
Duplex / Color / Paper size  - overrides for every print job
                               ("Printer default" = as configured in Windows)
Print immediately after adding - when checked, printing starts on its own
                                 as soon as files are added

All settings are remembered between runs (easytoprint.json next to the exe).

Other controls
--------------
Clear          - empties the queue except the file currently printing
Stop           - finishes after the current file (never cuts it in half)
× (per file)   - removes a single file from the queue

Requirements
------------
- Windows
- lib\pdfium.dll next to the program (included in this folder)
- Nothing else.
