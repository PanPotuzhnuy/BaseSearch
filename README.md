# Base Search 1.5.0

[![CI](https://github.com/PanPotuzhnuy/BaseSearch/actions/workflows/ci.yml/badge.svg)](https://github.com/PanPotuzhnuy/BaseSearch/actions/workflows/ci.yml)

Base Search is a local cross-platform desktop application for fast search and
practical analytics across large Excel datasets. It runs on **Windows, Linux,
and macOS**, imports spreadsheet files into a local database, builds a search
index, and lets users find, inspect, summarize, and export records without
fighting slow filters, freezing workbooks, or repeated manual searches in
Excel.

The first version was built for customs and import datasets. Version 1.5 turns
that into a broader table engine: take large tabular Excel exports, preserve
their real columns, store them locally, and make them searchable.

Base Search works offline. Source files, the database, and search results stay
on the user's computer.

## Features

- Import `.xlsx`, `.xlsb`, and `.xls` files, including ordinary tables that do
  not follow the customs schema.
- Search across product descriptions, companies, product codes, declaration
  numbers, trademarks, countries, and dates.
- Filter by year, product code, company, organization code, and country fields.
- Build flexible advanced searches with editable rules, all/any rule groups,
  exclusion rules, nested groups, range filters, empty/not-empty checks, and
  preserved extra columns from imported spreadsheets.
- View all imported source columns in the result table. Known customs fields
  keep their analytical meaning; unknown spreadsheet columns are preserved as
  dynamic fields and shown beside them.
- Hover abbreviated customs headers to see what fields such as `43`,
  `43_01`, `ФВ вал.контр`, `РФВ`, `РМВ`, `Вага по МД`, `Умови пост.`,
  `3001`, `3002`, and `9610` mean.
- Open a full details view for any result row.
- Use the **Questions** menu to jump from a current product, company, code,
  year, or country filter to the right analytical view: who imported it, what
  was moved, which countries dominate, how prices look, or how values changed
  by month.
- Open a separate Analytics tab for the current search/filter set: product
  rows, unique declarations, companies, value, net/gross weight, average value
  per kg, product codes, brands, countries, and price indicators. The Overview
  screen includes visual decision cards, detailed counters, and richer
  month-bar popups.
- See monthly dynamics on a bar chart: how value, row count, net weight, or
  average value per kg changed month to month for the matching rows. Hovering a
  bar shows a visual popup with the month, selected metric, declarations,
  value, weight, and price per kg.
- Compare who received/imported goods, who sent them, which product codes and
  brands dominate, where goods came from, and how much value/weight each group
  represents.
- Copy single values, whole rows, or selected rows back into Excel.
- Export search results to CSV or XLSX with the dynamic columns included.
- Open an optional local browser interface on `127.0.0.1` for searching,
  viewing tables, opening row cards, and reading analytics in a regular
  browser. This is still local: it is not an internet service.
- Keep the interface responsive while importing, searching, exporting, or
  cleaning the database.
- Use light/dark theme, adjustable UI scale, and an 11-language interface
  (English by default; also Ukrainian, German, Spanish, French, Polish,
  Portuguese, Romanian, Hungarian, Bulgarian, and Chinese).
- Run fully locally with no server and no cloud upload.

## Why Not Just Excel?

Excel is excellent for inspecting small and medium spreadsheets. It becomes
uncomfortable when the workflow turns into searching across many huge files:
opening takes time, filters lag, memory usage grows, and repeated searches are
manual.

Base Search changes the workflow:

1. Import the files once.
2. Build a local search index.
3. Search the indexed database instead of reopening every spreadsheet.

That makes repeated search and filtering much faster and more predictable.

## Quick Start

On macOS and Linux, the bundled `start.sh` script sets everything up in one
guided step — it shows each action in the terminal, installs what is missing,
builds the app, and opens it. The copy-paste blocks below use it.

### Windows

Run the prebuilt application — no install needed:

```text
dist\BaseSearch\BaseSearch.exe
```

For the browser interface, run:

```text
dist\BaseSearch\BaseSearch.exe --web
```

The app opens a local page such as `http://127.0.0.1:7832`. The page talks only
to the program running on the same computer; Excel files and the database are
not uploaded to the internet.

### macOS (copy-paste)

Paste this block into Terminal. The guided `start.sh` script checks your
system, installs the Rust toolchain if it is missing, builds the app while
narrating each step, and then launches it:

```bash
xcode-select --install 2>/dev/null || true
git clone https://github.com/PanPotuzhnuy/BaseSearch.git
cd BaseSearch
./start.sh
```

Run `./start.sh` again anytime to reopen the app — finished steps are skipped,
so it starts almost instantly. For the command-line tool use
`./run.sh cli stats data/base_search.db`.

### Linux (copy-paste)

The guided `start.sh` script installs the GUI build libraries (it detects
apt, dnf, or pacman), installs the Rust toolchain if missing, builds the app
step by step, and launches it. You only need git to clone first:

```bash
sudo apt-get update && sudo apt-get install -y git   # Debian / Ubuntu
git clone https://github.com/PanPotuzhnuy/BaseSearch.git
cd BaseSearch
./start.sh
```

On Fedora use `sudo dnf install -y git`; on Arch, `sudo pacman -S --needed git`.
Run `./start.sh` again anytime to reopen the app.

### Where the data lives

The local database is stored in a `data` folder next to the executable. When
that location is not writable (for example, a system-wide install), Base Search
falls back to `~/.base-search/` in the user's home directory. If the database
file does not exist, it is created automatically.

## Basic Workflow

1. Click **Import Excel** and select one or more spreadsheet files.
2. Wait for import and indexing to finish. Progress is shown in the status bar.
3. Type a query: product description, company name, product code, declaration
   number, trademark, or country.
4. Narrow results with filters when needed.
5. Use **+ Filter** and **Advanced** when a search needs several rules, such as
   "sender contains A or B", "exclude origin country CN", a year/date range, or
   a condition on an extra imported column.
6. Use **Questions** for guided shortcuts when you know the business question
   but do not want to choose the tab manually.
7. Open **Analytics** to understand the current query: who moved the goods,
   what goods dominate, where they came from, and what the value/weight picture
   looks like.
8. Double-click a row to open its full details.
9. Right-click a row for quick copy and quick filter actions.
10. Export the current result set to CSV or XLSX.

## Guided Questions

The **Questions** menu turns common trade-data questions into one-click
navigation. It reads the current search text and filters and offers relevant
shortcuts:

- for a product, brand, product code, or free-text search: who imported it,
  every company/EDRPOU, product-code and brand breakdowns, countries, prices,
  monthly dynamics, and company-by-month comparison;
- for a company or EDRPOU: the full company dossier, what it moved, who
  supplied it, which countries it worked with, monthly dynamics, and
  product-code-by-month comparison;
- for a year/country/current slice: largest companies, dominant goods,
  dominant routes, and price checks.

The menu is translated into all supported interface languages. It does not
guess legal responsibility for an import; it simply routes the user to the
matching recipient, sender, EDRPOU, goods, country, price, or pivot view.

## Browser Mode

Base Search can also run a local browser interface:

```text
dist\BaseSearch\BaseSearch.exe --web
```

or, in the Windows distribution folder, double-click:

```text
Open Browser Mode.cmd
```

The browser opens a local address such as `http://127.0.0.1:7832`. This is a
localhost interface, not an internet service: the database remains on the same
computer. The local API is protected by a per-session token sent in the request
header, so another page in the browser cannot casually read the local database.
Requests are served by a small fixed pool of worker threads that reuse their
database connections.

## Analytics Tab

Analytics always follows the same query and filters as the Results table. For
example, if the user searches for `Apple` and filters year `2024`, the Analytics
tab is calculated only for those matching rows.

The free-text search is intentionally broad: a word can match the product
description, company names, trademark, declaration number, product code, or
country fields. For business questions such as "show only the Apple trademark",
use the dedicated **Trademark** filter or click a trademark row in Analytics.
Those drill-down actions now apply field-specific filters instead of replacing
the whole query with another broad search.

The Analytics tab is split into focused sub-tabs, so each screen answers one
kind of question instead of cramming everything into one long page. A one-line
summary (rows · value · net weight · period) stays visible on every sub-tab.

| Sub-tab | What it answers |
|---|---|
| **Overview** | Visual decision cards for scale, documents, participants, goods, and geography; detailed counters for value, net/gross weight, quantity, companies, trademarks, and countries; plus a **monthly dynamics** bar chart. Switch the chart metric between source value, rows, net weight, and **value per kg**. Hover a bar for a popup with the full month. |
| **Companies** | Which organization codes dominate, who received/imported, and who sent. EDRPOU is shown first because it is more stable than company names with address variants. |
| **Goods** | Which product codes, brands, and product groups dominate. Codes can be grouped by HS level — **2 / 4 / 6 digits or full** — to see structure from chapter down to exact code. Brand totals depend on source files that actually contain trademark data. |
| **Countries** | Origin, dispatch, and trade countries for the matching shipments. Common country name/code variants are normalized, for example `CN` and `КИТАЙ` are grouped together. |
| **Prices** | Per price field: average, weighted average, **median, and the P25–P75 range** with the value count. Below the table, a **price-undervaluation scan** compares each row's value per kg with the median for its own product code, shows how many rows/codes were judged, the suspicious value, an estimated value gap, sample size, quartile range, and row-level risk details. |
| **Pivot** | A **cross-tab**: pick any dimension for rows and any for columns (company, EDRPOU, product code, trademark, origin/dispatch/trade country, month, year) and a value (source value, rows, net weight). The result is a heatmap with row, column, and grand totals; row/column labels drill into the Results table, and the whole matrix copies into Excel. |
| **Report** | A one-screen working report for the current query: headline totals, monthly dynamics, top companies, goods, countries, and price metrics. The report can be copied as text or exported as a print-ready HTML file that can be saved as PDF from the browser print dialog. |
| **Compare** | Compare the current query against another product/company text or another year while keeping the same filters. The app shows both sides and the delta for rows, declarations, value, net weight, value per kg, and EDRPOU count. |

Only the active sub-tab is calculated, which keeps the tab fast even on very
broad queries. Companies, Goods, and Countries are shown as side-by-side cards
with compact share rows — each row shows its value and share, the full numbers
(rows, declarations, companies, weight, average price) appear on hover, and
clicking a row applies the matching filter back to the Results table. Each card
has a **copy-table button** that puts the whole top list on the clipboard as a
tab-separated table, ready to paste into Excel or a report.

When the top list is not enough, each Companies, Goods, and Countries card also
has an **All** button. It opens an on-demand drill-down window for that exact
section: search inside the grouped list, sort by rows, declarations, companies,
value, weight, share, or value per kg, copy the visible rows to Excel, or click
a row to apply it as a Results filter. To keep the UI responsive on very broad
queries, the drill-down window loads up to the first 20,000 groups and says so
explicitly if that safety limit is reached.

Values and prices are shown exactly as they appear in the source files: in the
41-column layout the "value" can be in the contract currency rather than only
USD, which the tab notes explicitly so totals are not misread.

Numeric analytics accepts both comma and dot decimals. Customs weights with
three decimal places, such as `13804.656`, are treated as decimal values rather
than thousands-formatted integers.

To avoid heavy full-database grouping by accident, the Analytics tab asks for a
search term or filter before running large calculations.

## Company Dossier

Right-click any result row and choose **Company profile** (by EDRPOU) to open a
one-screen dossier for that importer: all name variants seen for the code,
headline numbers (rows, declarations, value, net weight, value per kg, distinct
product codes and suppliers), first-read highlights for the main good, sender,
and origin country, a monthly dynamics chart, and the company's top product
codes, suppliers, countries, and price metrics. Any card row drills back into the
filtered Results table, so "tell me everything about this company, then show me
the underlying lines" is a couple of clicks.

## Built-in Quick Guide

A short guide opens automatically on the first run and is available any time
from the **?** button or by pressing **F1**. It covers the basic interactions —
search syntax, double-click for the record card, right-click for quick filters
and the company profile, click / Ctrl+click / Shift+click selection with Ctrl+C
copy, the Analytics sub-tabs, and import/export/settings — in the chosen
interface language.

## Search Syntax

- `wine bottle` means both words must be present.
- `wine*` searches by word prefix.
- Numeric terms with 4+ digits, for example `8504`, are treated as prefixes,
  which is useful for product codes.
- Text filters are case-insensitive and support Cyrillic text.
- Use field filters when the meaning matters: **Trademark = Apple** is narrower
  than searching for `Apple` everywhere.
- Use **Advanced** for structured logic instead of SQL syntax: combine rules
  with **All rules** or **Any rule**, mark a rule or group as excluded, create
  nested groups, and search imported extra columns with the same rule builder.

## Supported Data

Base Search is designed for tabular spreadsheet exports. It works best when the
file contains one main table with a header row and consistent columns.

Supported input patterns include:

| Pattern | What it means |
|---|---|
| Any regular table | A spreadsheet table with a header row and consistent columns, even when none of the columns are known customs fields. |
| Standard customs table | A regular customs spreadsheet table with recognizable columns. |
| Extended table | A standard table with extra columns; the extra columns are preserved, searchable, shown in results, and exported. |
| Registry-style export | A table where some logical fields are split across multiple columns. |
| Header after title rows | A file with title or metadata rows before the actual table header. |

Columns that do not match the known customs schema are not discarded: they are
kept with each row, indexed for full-text search, shown in the desktop table,
served through the browser interface, listed on the record card, available in
Advanced Search as extra fields, and included in CSV/XLSX export.

If a file cannot be recognized as a customs export, Base Search imports it as a
generic table instead of rejecting it. Customs-specific analytics use only the
recognized semantic fields; generic columns remain searchable and exportable.

## Performance

Performance depends on the user's hardware, disk speed, file format, dataset
shape, and query breadth. An SSD is strongly recommended for large databases.

In development testing, Base Search handled multi-million-row datasets locally:

| Operation type | Expected behavior |
|---|---|
| Import | Usually limited by Excel parsing speed and disk writes. Large files may take minutes. |
| Repeat import of the same file | Fast, because identical files are skipped by content hash. |
| Narrow search | Usually interactive after indexing. |
| Very broad search | Slower, because the app must count and page through many matching rows. |
| Analytics | Calculated for the active query/filter. Broad analytics depends on result size and disk speed. |
| CSV export | Recommended for very large result sets. |
| XLSX export | Convenient for smaller exports, but limited by Excel worksheet size. |

These are not universal benchmark guarantees. A weak HDD-based PC and a modern
SSD desktop will behave very differently.

### Database Maintenance

SQLite databases can temporarily look larger than the visible data after large
imports, migrations, cancelled imports, or heavy deletes. Two normal SQLite
mechanisms are involved:

- the `*.db-wal` file, which stores recent writes before they are checkpointed;
- free pages inside the main `*.db` file, which SQLite can reuse but the
  operating system does not see as free disk space until `VACUUM` rewrites the
  file.

The command-line tool can inspect and compact local storage:

```powershell
base-search-cli stats data/base_search.db
base-search-cli compact data/base_search.db
base-search-cli compact data/base_search.db --vacuum
```

`compact` without `--vacuum` performs a safe WAL checkpoint and is usually
quick. `compact --vacuum` keeps the data but rewrites the database file to
return internal free pages to the filesystem; on multi-gigabyte databases this
can take a long time and should be run only after closing other Base Search
windows.

## Command-Line Utility

The distribution includes a small diagnostic tool for checking data without the
graphical interface:

```powershell
base-search-cli stats  <db>
base-search-cli compact <db> [--vacuum]
base-search-cli peek   <file.xlsx|file.xlsb>
base-search-cli import <db> <file.xlsx|file.xlsb> [...]
base-search-cli search <db> [query...] [--limit N] [--year Y] [--code C]
base-search-cli analytics <db> [query...] [--year Y] [--code C]
base-search-cli export <db> <out.csv|out.xlsx> [query...]
base-search-cli web [db] [--host 127.0.0.1] [--port 7832] [--no-open]
```

The GUI is the primary user interface. The CLI is intended for troubleshooting,
benchmarking, and quick verification.

## Build From Source

Requirements:

- Rust stable (1.96+)
- **Windows:** MSVC toolchain (Visual Studio Build Tools)
- **Linux:** `build-essential`, `pkg-config`, `libxkbcommon-dev`,
  `libwayland-dev` (X11 and Wayland are both supported at runtime)
- **macOS:** Xcode Command Line Tools

Commands (identical on every platform):

```bash
cargo test
cargo build --release
```

Release binaries are created in `target/release/`: `BaseSearch` and
`base-search-cli` (with `.exe` on Windows). On macOS and Linux two helper
scripts are bundled:

```bash
./start.sh            # guided first run: checks tools, installs, builds, launches
./run.sh              # quiet build (release) and run, for repeat use
./run.sh cli stats data/base_search.db   # run the command-line tool
```

Continuous integration builds and tests every commit on Windows, Linux, and
macOS and publishes downloadable binaries as workflow artifacts. macOS and
Linux CI binaries are unsigned — on macOS, clear the quarantine flag once with
`xattr -d com.apple.quarantine ./BaseSearch` (or right-click → Open), or just
build from source as above.

## Architecture

- **Rust** for the application core and native executables on every platform.
- **egui/eframe** for the desktop interface.
- **calamine** for reading Excel files.
- **SQLite** for local storage in a single database file.
- **SQLite FTS5** for fast full-text search.
- **SQLite aggregate queries** for local analytics over the current result set.
- **Built-in localhost web interface** for optional browser-based search and
  analytics without uploading data.
- **xxhash** for duplicate detection.
- **CSV and XLSX writers** for exporting results.

The database is intentionally stored outside the executable because real
datasets can grow to many gigabytes.

## Privacy

Base Search has no cloud backend and does not upload user files. It reads
selected local spreadsheets and writes a local SQLite database beside the
application executable.

## Changelog

### 1.5.0

- **Universal table import.** Excel files no longer need to match the customs
  schema. If no known layout is detected, Base Search imports the sheet as a
  generic table and preserves every source column.
- **Dynamic result columns.** Desktop results, browser results, row cards, page
  CSV export, and full CSV/XLSX export now include imported extra columns.
- **Universal Advanced Search fields.** Extra spreadsheet headers remain
  available as typed search fields, with inferred text/code/date/number/country
  behavior where possible.
- **Source-order preservation.** Extra columns are listed in the order they
  first appear in the imported file instead of being alphabetically reordered.

### 1.4.1

- **Database startup hotfix.** Existing large databases are migrated in place
  without forcing a multi-gigabyte FTS rebuild when the indexed text is already
  compatible.
- **SQLite maintenance CLI.** `base-search-cli stats` now reports file sizes,
  WAL size, and SQLite free pages. `base-search-cli compact` can truncate WAL
  safely, and `compact --vacuum` can rewrite the database to return internal
  free pages to the filesystem.
- **Release hygiene.** Local release packages and multi-gigabyte databases are
  kept out of git, while the Windows distribution binaries are rebuilt.

### 1.4.0

- **Advanced Search 1.4.** Added a flexible query builder for the desktop app:
  editable rule chips, all/any groups, exclusion rules and groups, nested
  groups, range filters, empty/not-empty checks, and extra-column conditions.
- **Universal query model.** Search rules are stored as a structured AST and
  compiled to parameterized SQLite queries, while the existing flat filters and
  full-text search behavior remain compatible.
- **Saved and recent advanced searches.** Advanced queries are serialized as V2
  saved/recent search data while legacy saved searches still decode.
- **Field catalog.** The app now builds a searchable field catalog from known
  record fields, the virtual year field, and extra headers discovered during
  import.
- **Localized advanced-search interface.** New search-builder labels, menus,
  operators, hints, and rule summaries are translated across all 11 supported
  interface languages.

### 1.3.0

- **Universal column capture.** Columns beyond the known schema are now kept with
  each imported row, included in full-text search, and listed on the record card.
  Differently shaped customs exports import without losing data.
- **Browser mode hardening.** The local web interface now runs on a fixed pool of
  worker threads that reuse their database connections, parses requests more
  strictly, and authenticates with a per-session token sent in the request header
  instead of the URL.

### 1.2.0

- **Smart Questions menu.** Context-aware business questions now route the user
  directly from the current product, company, EDRPOU, year, or country filter
  into the right analytical view: companies, goods, countries, prices, monthly
  dynamics, pivots, full group lists, or company dossiers. The menu is
  translated for all supported interface languages.
- **Expanded column glossary.** Abbreviated customs table headers now show
  hover explanations for technical fields such as `43`, `43_01`, `Вага по МД`,
  `Умови пост.`, `Місце пост`, `Вага.один.`, `Вага різн.`, `3001`, `3002`,
  `9610`, `пільгова`, `повна`, and the value/price columns.
- **Local browser mode.** The app can start a local web interface on
  `127.0.0.1` with token-protected API routes. The Windows distribution
  includes `Open Browser Mode.cmd` for a double-click launch.
- **Search and analytics workflow polish.** Recent and saved searches preserve
  full filters, broad structural searches page faster, and analytics explains
  how rows, declarations, totals, shares, and price metrics are calculated.
- **Report and Compare modes.** Analytics now includes a printable Report view
  for the current query and a Compare view for checking another product,
  company, or year against the current selection. Reports export as Unicode-safe
  HTML that can be saved as PDF from the browser print dialog.
- **Company dossier polish.** Company profiles now open with a clearer identity
  block and first-read highlights for the main good, sender, and country before
  the detailed sections.

### 1.1.1

- **Guided first-run script for macOS and Linux.** `./start.sh` narrates each
  step in the terminal — it checks the OS, installs the Rust toolchain (and the
  Linux GUI libraries) only when missing, builds the app, and launches it — so
  a non-technical user can set everything up with a single command.
- **11 interface languages, English by default.** The app now starts in
  English everywhere; the interface is also available in Ukrainian, German,
  Spanish, French, Polish, Portuguese, Romanian, Hungarian, Bulgarian, and
  Chinese, switchable any time in Settings.
- **CJK font fallback.** Base Search uses system CJK fonts when available so
  the Chinese interface renders without bundling a large font into the binary.
- **Localization cleanup.** UI strings that used to be hardcoded in the app are
  centralized in the translation layer, so every supported language gets the
  analytics labels, tooltips, pivot text, price labels, and quick guide.

### 1.1

- **Cross-platform support.** Base Search now builds and runs on Windows,
  Linux (X11 and Wayland), and macOS. System fonts are picked per OS with a
  safe built-in fallback; the database location falls back to the home
  directory when the install folder is read-only.
- **Analytics restructured into focused sub-tabs.** Overview, Companies,
  Goods, Countries, Prices, and Pivot are separate screens instead of one
  long scrolling page, with a persistent one-line summary. Only the active
  sub-tab is calculated, so the tab stays fast on broad queries.
- **Pivot cross-tab.** Cross-tabulate any dimension by any other (company,
  EDRPOU, code, trademark, country, month, year) for value / rows / net
  weight, as a heatmap with totals; labels drill into results and the matrix
  copies into Excel.
- **Company dossier.** A one-screen profile per importer (by EDRPOU) with
  name variants, headline numbers, monthly dynamics, and top products,
  suppliers, and origin countries; opened from the row context menu.
- **Price-undervaluation scan.** Flags declarations priced per kg far below
  the median for their own product code — a customs undervaluation signal.
- **Deeper analytics for real work:** value-per-kg metric on the
  monthly chart (price trends and dumping), HS-code grouping by 2/4/6/full
  digits, median and P25–P75 in the price table instead of misleading
  min/max, a copy-table button on every card (pastes into Excel), and an
  explicit note about contract-currency values.
- **Continuous integration.** Every commit is tested and built on all three
  platforms; binaries are published as workflow artifacts.

### 1.0

- Initial public release: streaming Excel import (`.xlsx`/`.xlsb`/`.xls`),
  full-text search with FTS5, filters, analytics tab with KPI tiles and
  clickable share charts, two-level duplicate protection, CSV/XLSX export,
  UA/RU/EN interface, light/dark theme.

## License

Base Search is released under the MIT License. You can use, copy, modify, and
redistribute the application and source code, as long as the copyright notice
and license text are included.
