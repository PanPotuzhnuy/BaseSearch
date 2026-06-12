Base Search 1.0
===============

How to run
----------
Double-click BaseSearch.exe.

What this folder contains
-------------------------
- BaseSearch.exe: the desktop application.
- base-search-cli.exe: optional command-line diagnostics.
- data/: local database folder. It is created and used on the user's computer.

Basic workflow
--------------
1. Open BaseSearch.exe.
2. Click Import Excel and select .xlsx, .xlsb, or .xls files.
3. Search by product, company, product code, declaration number, country, or trademark.
4. Use filters for year, code, EDRPOU, company, and country fields.
5. Open Analytics to understand the current search: rows, declarations,
   companies, value, net/gross weight, average value per kg, product codes,
   brands, countries, and price indicators.
   The Analytics tab is split into Companies, Goods, Countries, and Prices.
   Click an analytics row to apply its filter back to the results.
6. Double-click a row to see all imported fields.
7. Export matching rows to CSV or XLSX when needed.

Privacy
-------
Base Search works locally. It does not upload Excel files or databases to a
server. Imported data is stored in data/base_search.db next to the program.
