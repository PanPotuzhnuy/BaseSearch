Base Search 1.5.0
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

Database maintenance
--------------------
If data/base_search.db becomes much larger after big imports, close other
Base Search windows and run:

base-search-cli.exe stats data\base_search.db
base-search-cli.exe compact data\base_search.db

The compact command safely truncates the SQLite WAL file. For deeper
compaction, run:

base-search-cli.exe compact data\base_search.db --vacuum

The --vacuum mode keeps records but rewrites the database file. It can take a
long time on multi-gigabyte databases.

Basic workflow
--------------
1. Open BaseSearch.exe.
2. Click Import Excel and select .xlsx, .xlsb, or .xls files.
3. For customs exports, search by product, company, product code, declaration
   number, country, or trademark. For ordinary tables, search by any text or
   value from the imported columns.
4. Use filters for year, code, EDRPOU, company, and country fields when those
   semantic fields exist.
5. Use + Filter and Advanced when a search needs several rules, any/all logic,
   excluded rules, ranges, empty/not-empty checks, or extra imported columns.
6. Use Questions when you want the app to choose the right analytics view.
7. Open Analytics to understand the current search: rows, declarations,
   companies, value, net/gross weight, average value per kg, product codes,
   brands, countries, and price indicators.
   The Analytics tab is split into Overview, Companies, Goods, Countries,
   Prices, Pivot, Report, and Compare. Report creates a working summary that
   can be copied or exported as print-ready HTML for saving as PDF from the
   browser print dialog. Compare checks another product/company/year against
   the current selection. Overview has decision cards and richer month-bar
   popups; Prices has a stronger undervaluation scan with judged samples,
   suspicious value, estimated gap, and row-level risk details. Click an
   analytics row to apply its filter back to the results.
8. Double-click a row to see all imported fields; right-click for quick
   filters and the company profile.
9. Export matching rows to CSV or XLSX when needed.

Advanced search
---------------
The desktop app includes a flexible rule builder. Use it for searches such as
"sender contains A or B", "exclude origin country CN", "year/date is between",
or "extra imported column is not empty". Rules can be combined with all/any
groups and nested when a search needs more structure.

Universal tables
----------------
Base Search 1.5.0 can import regular Excel tables even when they do not follow
the customs schema. Unknown columns are preserved as dynamic fields, included
in full-text search, shown in the desktop and browser result tables, available
in Advanced Search, listed on the row card, and exported to CSV/XLSX.

Privacy
-------
Base Search works locally. It does not upload Excel files or databases to a
server. Imported data is stored in data/base_search.db next to the program.
