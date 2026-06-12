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

The top block, **Answer for current query**, shows the main numbers:

- product rows, which are table rows, not declaration count;
- unique declarations;
- recipients, senders, and organization codes;
- total value from the source value field when it is present;
- net and gross weight;
- average value per net kilogram;
- counts of product codes, trademarks, and countries.

The lower sections are compact charts instead of overloaded dashboards:

| Section | What it answers |
|---|---|
| Monthly dynamics | How value, rows, or net weight changed month to month: seasonality, spikes, when imports started or stopped. Switch the metric above the chart; hover a bar for the full numbers. |
| Companies | Who received/imported, who sent, and which organization codes are most important. |
| Goods | Which product codes, brands, and short product groups dominate. |
| Countries | Origin, dispatch, and trade countries for the matching shipments. |
| Prices | Average, weighted, minimum, and maximum price indicators where source fields are filled. |

Each row shows its share, value, weight, row count, declaration count, company
count, and average value per kg. Clicking a row applies the matching filter back
to the Results table.

To avoid heavy full-database grouping by accident, the Analytics tab asks for a
search term or filter before running large calculations.

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
- **Monthly dynamics chart.** The Analytics tab opens with a bar chart of the
  matched rows grouped by month, switchable between value ($), row count, and
  net weight, with hover details for every month.
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
