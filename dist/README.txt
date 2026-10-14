EasyToPrint — bulk PDF printing
===============================

Getting started
---------------
1) Make sure BOTH files sit in the same folder:
   - easytoprint.exe
   - pdfium.dll        (the PDF rendering library)

2) Run easytoprint.exe.

3) Drag & drop PDF files onto the light drop zone
   (or click the zone to browse for files).
   You can keep adding files while printing is in progress.

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
Printer                        - pick a printer from the system list
                                 ("Default" = the Windows default printer)
Print immediately after adding - when checked, printing starts on its own
                                 as soon as files are added

Other controls
--------------
Clear          - empties the queue except the file currently printing
Stop           - finishes after the current file (never cuts it in half)
× (per file)   - removes a single file from the queue

Requirements
------------
- Windows
- pdfium.dll next to the program (included in this folder)
- Nothing else.
