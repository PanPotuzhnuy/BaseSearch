# Base Search 1.5.0

[![CI](https://github.com/IvanK577/BaseSearch/actions/workflows/ci.yml/badge.svg)](https://github.com/IvanK577/BaseSearch/actions/workflows/ci.yml)

Base Search is a local desktop application for fast search, filtering,
analytics, and export across large Excel datasets.

It is built for people who have many large spreadsheet files and need to work
with them as one searchable database instead of opening heavy workbooks one by
one in Excel. The original focus was customs and trade data, but version 1.5.0
can also import ordinary tabular Excel files and preserve their real source
columns.

Base Search runs locally. It does not upload spreadsheets, search results, or
the database to a cloud service.

## What It Does

- Imports `.xlsx`, `.xlsb`, and `.xls` files into one local SQLite database.
- Preserves known trade/customs fields and unknown spreadsheet columns.
- Builds a full-text search index for fast repeated searches.
- Searches across products, companies, codes, declaration numbers, countries,
  trademarks, and generic imported columns.
- Supports advanced search rules: all/any groups, excluded rules, nested
  groups, ranges, empty/not-empty checks, and filters over extra columns.
- Shows paged results instead of trying to render millions of rows at once.
- Opens a full details card for any row.
- Provides analytics for the current query and filters.
- Exports results to CSV or XLSX.
- Includes an optional local browser interface on `127.0.0.1`.
- Works offline on Windows, macOS, and Linux.

## Typical Use Cases

- Search across many Excel exports as one dataset.
- Find all rows related to a product, brand, code, company, country, or year.
- Compare which companies, product codes, brands, or countries dominate a
  selected result set.
- Inspect suspicious prices or unusual value-per-weight patterns.
- Prepare filtered CSV/XLSX extracts for further work in Excel, BI tools, or
  reports.
- Use generic Excel tables as searchable local data without writing SQL.

## Why Not Just Excel?

Excel is strong for viewing and editing spreadsheets. It becomes inconvenient
when the workflow is mostly:

1. Open a very large file.
2. Wait for filters or search.
3. Repeat the same search in several other files.
4. Copy matching rows into a new workbook.

Base Search changes that workflow:

1. Import the files once.
2. Let the app build a local database and search index.
3. Search, filter, analyze, and export from the indexed database.

This is especially useful when the same dataset is searched many times.

## Quick Start

### Windows

Run the prebuilt application from the distribution folder:

```text
dist\BaseSearch\BaseSearch.exe
```

To open the local browser interface:

```text
dist\BaseSearch\BaseSearch.exe --web
```

or double-click:

```text
dist\BaseSearch\Open Browser Mode.cmd
```

The browser opens a local address such as `http://127.0.0.1:7832`. This is not a
hosted web service. The page talks to the Base Search process running on the
same computer.

### macOS

Build and run from source:

```bash
xcode-select --install 2>/dev/null || true
git clone https://github.com/IvanK577/BaseSearch.git
cd BaseSearch
./start.sh
```

The `start.sh` script checks the environment, installs missing Rust tooling
when needed, builds the app, and launches it.

### Linux

Install Git first, then run the guided setup:

```bash
sudo apt-get update && sudo apt-get install -y git
git clone https://github.com/IvanK577/BaseSearch.git
cd BaseSearch
./start.sh
```

On Fedora use `sudo dnf install -y git`. On Arch use
`sudo pacman -S --needed git`.

## Data Location

Base Search stores its database outside the executable.

Default locations:

- distribution folder: `data/base_search.db`
- fallback home folder: `~/.base-search/base_search.db`

Large real-world databases can grow to many gigabytes. Keeping the database
outside the executable makes updates and backups simpler.

## Basic Workflow

1. Open Base Search.
2. Click **Import Excel** and choose one or more files.
3. Wait until import and indexing finish.
4. Type a search query or add filters.
5. Use **Advanced** for structured search logic.
6. Review the result table.
7. Open row details when needed.
8. Open **Analytics** for summaries and breakdowns.
9. Export matching rows to CSV or XLSX.

## Universal Table Import

Base Search 1.5.0 is no longer limited to one fixed spreadsheet layout.

If a file matches a known customs/trade layout, the app maps fields such as
date, company, product code, country, value, weight, and price indicators into
semantic columns.

If a file does not match a known layout, Base Search imports it as a generic
table:

- the detected header row becomes the column list;
- every source column is preserved;
- values are indexed for full-text search;
- extra fields are visible in the result table;
- extra fields are available in Advanced Search;
- CSV/XLSX export includes the dynamic columns.

Customs-specific analytics use recognized semantic fields when they exist.
Generic columns remain searchable, filterable, visible, and exportable.

## Search

Base Search supports two search styles.

### Simple Search

Use the main search box for fast broad search:

```text
apple
8504
wine bottle
company name
```

Rules:

- multiple words must all be present;
- `word*` searches by prefix;
- numeric terms with 4 or more digits are treated as code prefixes;
- text matching is case-insensitive;
- field filters are better when the meaning matters.

For example, searching `Apple` everywhere is broad. Filtering by trademark or
company is narrower and more precise.

### Advanced Search

Use Advanced Search for structured questions:

- sender contains A or B;
- origin country is not CN;
- year is between 2024 and 2026;
- value is greater than a threshold;
- an imported extra column is empty or not empty;
- several groups of rules should match with all/any logic.

Advanced Search is designed for users who need flexible filtering without
writing SQL.

## Analytics

Analytics follows the current search and filters. If the Results table is
showing a filtered subset, Analytics is calculated for the same subset.

Available views include:

| View | Purpose |
|---|---|
| Overview | Headline totals, declarations, companies, value, weight, quantity, countries, and monthly dynamics. |
| Companies | Top organization codes, recipients/importers, and senders. |
| Goods | Product codes, brands, and product groups. |
| Countries | Origin, dispatch, and trade countries. |
| Prices | Average and weighted price metrics, medians, quartiles, and possible undervaluation checks. |
| Pivot | Cross-tab analysis by company, code, country, month, year, or other supported dimensions. |
| Report | A compact working report that can be copied or saved as print-ready HTML. |
| Compare | Compare the current result set with another query or year. |

For very broad data, Base Search avoids running heavy analytics on an empty
global query by accident. Add a query or filter first.

## Browser Mode

Browser mode exposes the same local database through a localhost interface:

```text
BaseSearch.exe --web
```

It is useful when a browser-based table and analytics view is more convenient
than the native desktop UI.

Security notes:

- the server binds to localhost by default;
- API routes use a per-session token;
- files and database content stay on the same machine;
- this is not a multi-user hosted server.

## Export

Base Search can export the current result set to:

- CSV for large exports and compatibility with most tools;
- XLSX for smaller Excel-friendly exports.

XLSX export is limited by Excel worksheet limits. CSV is recommended for very
large result sets.

## Command-Line Tool

The distribution includes `base-search-cli` for diagnostics, maintenance, and
automation:

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

The desktop app is the primary interface. The CLI is mainly for verification,
batch work, troubleshooting, and database maintenance.

## Database Maintenance

SQLite can temporarily use extra disk space after large imports, cancelled
imports, deletes, or migrations. This is normal.

Useful commands:

```powershell
base-search-cli stats data/base_search.db
base-search-cli compact data/base_search.db
base-search-cli compact data/base_search.db --vacuum
```

`compact` checkpoints and truncates the WAL file. `compact --vacuum` rewrites
the database to return unused pages to the filesystem. Vacuuming a large
database can take a long time and should be done after closing other Base
Search windows.

## Performance Notes

Performance depends on:

- CPU speed;
- SSD/HDD speed;
- Excel file format;
- number of rows and columns;
- query breadth;
- available RAM;
- whether analytics or export is running.

Narrow searches after indexing are usually interactive. Import speed is often
limited by Excel parsing and disk writes. Very broad analytics and large exports
depend heavily on database size and hardware.

## Build From Source

Requirements:

- Rust stable
- Windows: MSVC toolchain
- macOS: Xcode Command Line Tools
- Linux: build tools, `pkg-config`, `libxkbcommon-dev`, and Wayland/X11 GUI
  libraries

Build and test:

```bash
cargo test
cargo build --release
```

Release binaries are created in `target/release/`:

- `BaseSearch` / `BaseSearch.exe`
- `base-search-cli` / `base-search-cli.exe`

Helper scripts for macOS and Linux:

```bash
./start.sh
./run.sh
./run.sh cli stats data/base_search.db
```

## Architecture

Base Search is built with:

- Rust for the application core and native executables;
- egui/eframe for the desktop interface;
- calamine for reading Excel files;
- SQLite for local storage;
- SQLite FTS5 for full-text search;
- SQLite aggregate queries for analytics;
- a small localhost web server for browser mode;
- xxhash for duplicate detection;
- CSV and XLSX writers for export.

The current architecture is a local single-machine application. A hosted or
multi-user server edition would require a separate deployment model and is not
part of the current release.

## Privacy

Base Search has no cloud backend. It reads selected local files and writes a
local database. Users are responsible for protecting the files, exported
reports, and database on their own machine.

## Limitations

- Base Search is not a spreadsheet editor.
- It does not replace legal, customs, accounting, or compliance review.
- Generic tables are searchable and exportable, but semantic analytics require
  recognizable fields such as dates, values, weights, companies, codes, or
  countries.
- Browser mode is local-only, not a shared web application.
- Very large databases still need enough disk space and a reasonably fast SSD.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.

## License

Base Search is released under the MIT License. You can use, copy, modify, and
redistribute the application and source code as long as the copyright notice
and license text are included.
