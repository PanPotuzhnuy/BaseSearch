Base Search 1.2.0
===============

How to run
----------
Double-click BaseSearch.exe.

A short built-in guide opens on first run; reopen it any time with the
"?" button or the F1 key.

Browser mode
------------
Double-click "Open Browser Mode.cmd", or run:

BaseSearch.exe --web

This opens a local page in the browser, usually http://127.0.0.1:7832.
It is not an internet site: the database stays on this computer.

Questions menu
--------------
Use the Questions button after entering a product, company, EDRPOU, year, or
country. It jumps straight to useful analytics: who imported it, what goods
dominate, which countries/routes are involved, how prices look, monthly
dynamics, pivots, full company lists, or a company dossier.

Column hints
------------
Hover table headers to decode abbreviated customs fields such as 43, 43_01,
FV, RFV, RMV, Vaga po MD, Umovy post., Mistse post, 3001, 3002, and 9610.

What this folder contains
-------------------------
- BaseSearch.exe: the desktop application.
- base-search-cli.exe: optional command-line diagnostics.
- Open Browser Mode.cmd: starts the local browser interface.
- data/: local database folder. It is created and used on the user's computer.

Basic workflow
--------------
1. Open BaseSearch.exe.
2. Click Import Excel and select .xlsx, .xlsb, or .xls files.
3. Search by product, company, product code, declaration number, country, or trademark.
4. Use filters for year, code, EDRPOU, company, and country fields.
5. Use Questions when you want the app to choose the right analytics view.
6. Open Analytics to understand the current search: rows, declarations,
   companies, value, net/gross weight, average value per kg, product codes,
   brands, countries, and price indicators.
   The Analytics tab is split into Overview, Companies, Goods, Countries,
   Prices, and Pivot. Click an analytics row to apply its filter back to the
   results.
7. Double-click a row to see all imported fields; right-click for quick
   filters and the company profile.
8. Export matching rows to CSV or XLSX when needed.

Privacy
-------
Base Search works locally. It does not upload Excel files or databases to a
server. Imported data is stored in data/base_search.db next to the program.
