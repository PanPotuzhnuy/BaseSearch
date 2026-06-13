# Base Search 1.1

[![CI](https://github.com/PanPotuzhnuy/BaseSearch/actions/workflows/ci.yml/badge.svg)](https://github.com/PanPotuzhnuy/BaseSearch/actions/workflows/ci.yml)

Base Search is a local cross-platform desktop application for fast search and
practical analytics across large Excel datasets. It runs on **Windows, Linux,
and macOS**, imports spreadsheet files into a local database, builds a search
index, and lets users find, inspect, summarize, and export records without
fighting slow filters, freezing workbooks, or repeated manual searches in
Excel.

The first version was built for customs and import datasets, but the core idea
is broader: take large tabular Excel exports, store them locally, and make them
searchable.

Base Search works offline. Source files, the database, and search results stay
on the user's computer.

## Features

- Import `.xlsx`, `.xlsb`, and `.xls` files.
- Search across product descriptions, companies, product codes, declaration
  numbers, trademarks, countries, and dates.
- Filter by year, product code, company, organization code, and country fields.
- View all imported source columns in the result table, including value, price,
  weight, rate, and technical customs fields when they exist in the source data.
- Open a full details view for any result row.
- Open a separate Analytics tab for the current search/filter set: product
  rows, unique declarations, companies, value, net/gross weight, average value
  per kg, product codes, brands, countries, and price indicators.
- See monthly dynamics on a bar chart: how value, row count, or net weight
  changed month to month for the matching rows — seasonality, spikes, and
  when a company started or stopped importing.
- Compare who received/imported goods, who sent them, which product codes and
  brands dominate, where goods came from, and how much value/weight each group
  represents.
- Copy single values, whole rows, or selected rows back into Excel.
- Export search results to CSV or XLSX.
- Keep the interface responsive while importing, searching, exporting, or
  cleaning the database.
- Use light/dark theme, adjustable UI scale, and RU/UA/EN interface language.
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

**Windows.** Run the prebuilt application:

```text
dist\BaseSearch\BaseSearch.exe
```

**Linux / macOS.** Build from source (see below) or download the binaries from
the CI artifacts, then run `BaseSearch`.

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
5. Open **Analytics** to understand the current query: who moved the goods,
   what goods dominate, where they came from, and what the value/weight picture
   looks like.
6. Double-click a row to open its full details.
7. Right-click a row for quick copy and quick filter actions.
8. Export the current result set to CSV or XLSX.

## Analytics Tab

Analytics always follows the same query and filters as the Results table. For
example, if the user searches for `Apple` and filters year `2024`, the Analytics
tab is calculated only for those matching rows.

The Analytics tab is split into focused sub-tabs, so each screen answers one
kind of question instead of cramming everything into one long page. A one-line
summary (rows · value · net weight · period) stays visible on every sub-tab.

| Sub-tab | What it answers |
|---|---|
| **Overview** | Headline numbers (rows, declarations, companies, value, weight, average $/kg, distinct codes and countries) plus a **monthly dynamics** bar chart. Switch the chart metric between value ($), rows, net weight, and **average price ($/kg)** — the price line is what reveals price trends and dumping. Hover a bar for the full month. |
| **Companies** | Who received/imported, who sent, and which organization codes dominate. |
| **Goods** | Which product codes, brands, and product groups dominate. Codes can be grouped by HS level — **2 / 4 / 6 digits or full** — to see structure from chapter down to exact code. |
| **Countries** | Origin, dispatch, and trade countries for the matching shipments. |
| **Prices** | Per price field: average, weighted average, **median, and the P25–P75 range** with the value count. Below the table, a **price-undervaluation scan** lists declarations priced per kg far below the median for their own product code — the classic signal of customs undervaluation (or a data-entry error). |
| **Pivot** | A **cross-tab**: pick any dimension for rows and any for columns (company, EDRPOU, product code, trademark, origin/dispatch/trade country, month, year) and a value (value $, rows, net weight). The result is a heatmap with row, column, and grand totals; row/column labels drill into the Results table, and the whole matrix copies into Excel. |

Only the active sub-tab is calculated, which keeps the tab fast even on very
broad queries. Companies, Goods, and Countries are shown as side-by-side cards
with compact share rows — each row shows its value and share, the full numbers
(rows, declarations, companies, weight, average price) appear on hover, and
clicking a row applies the matching filter back to the Results table. Each card
has a **copy-table button** that puts the whole top list on the clipboard as a
tab-separated table, ready to paste into Excel or a report.

Values and prices are shown exactly as they appear in the source files: in the
41-column layout the "value" can be in the contract currency rather than only
USD, which the tab notes explicitly so totals are not misread.

To avoid heavy full-database grouping by accident, the Analytics tab asks for a
search term or filter before running large calculations.

## Company Dossier

Right-click any result row and choose **Company profile** (by EDRPOU) to open a
one-screen dossier for that importer: all name variants seen for the code,
headline numbers (rows, declarations, value, net weight, average $/kg, distinct
product codes and suppliers), a monthly dynamics chart, and the company's top
product codes, suppliers, and origin countries. Any card row drills back into
the filtered Results table, so "tell me everything about this company, then show
me the underlying lines" is a couple of clicks.

## Search Syntax

- `wine bottle` means both words must be present.
- `wine*` searches by word prefix.
- Numeric terms with 4+ digits, for example `8504`, are treated as prefixes,
  which is useful for product codes.
- Text filters are case-insensitive and support Cyrillic text.

## Supported Data

Base Search is designed for tabular spreadsheet exports. It works best when the
file contains one main table with a header row and consistent columns.

Supported input patterns include:

| Pattern | What it means |
|---|---|
| Standard table | A regular spreadsheet table with recognizable columns. |
| Extended table | A standard table with additional columns that can be ignored safely. |
| Registry-style export | A table where some logical fields are split across multiple columns. |
| Header after title rows | A file with title or metadata rows before the actual table header. |

If a file cannot be recognized, Base Search reports which required columns are
missing instead of crashing.

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

## Command-Line Utility

The distribution includes a small diagnostic tool for checking data without the
graphical interface:

```powershell
base-search-cli stats  <db>
base-search-cli peek   <file.xlsx|file.xlsb>
base-search-cli import <db> <file.xlsx|file.xlsb> [...]
base-search-cli search <db> [query...] [--limit N] [--year Y] [--code C]
base-search-cli analytics <db> [query...] [--year Y] [--code C]
base-search-cli export <db> <out.csv|out.xlsx> [query...]
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
`base-search-cli` (with `.exe` on Windows). Continuous integration builds and
tests every commit on Windows, Linux, and macOS and publishes downloadable
binaries as workflow artifacts.

## Architecture

- **Rust** for the application core and native executables on every platform.
- **egui/eframe** for the desktop interface.
- **calamine** for reading Excel files.
- **SQLite** for local storage in a single database file.
- **SQLite FTS5** for fast full-text search.
- **SQLite aggregate queries** for local analytics over the current result set.
- **xxhash** for duplicate detection.
- **CSV and XLSX writers** for exporting results.

The database is intentionally stored outside the executable because real
datasets can grow to many gigabytes.

## Privacy

Base Search has no cloud backend and does not upload user files. It reads
selected local spreadsheets and writes a local SQLite database beside the
application executable.

## Changelog

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
- **Deeper analytics for real work:** average-price ($/kg) metric on the
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
