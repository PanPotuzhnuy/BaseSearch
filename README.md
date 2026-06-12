# Base Search 1.0

Base Search is a local Windows desktop application for fast search and basic
analytics across large Excel datasets. It imports spreadsheet files into a local
database, builds a search index, and lets users find, inspect, summarize, and
export records without fighting slow filters, freezing workbooks, or repeated
manual searches in Excel.

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
- Calculate analytics for the current search/filter set: row count, unique
  companies, total value, weight, quantity, and top recipients, senders,
  trademarks, product codes, and origin countries.
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

Run the application:

```text
dist\BaseSearch\BaseSearch.exe
```

The local database is stored next to the executable:

```text
dist\BaseSearch\data\base_search.db
```

If the database file does not exist, Base Search creates it automatically.

## Basic Workflow

1. Click **Import Excel** and select one or more spreadsheet files.
2. Wait for import and indexing to finish. Progress is shown in the status bar.
3. Type a query: product description, company name, product code, declaration
   number, trademark, or country.
4. Narrow results with filters when needed.
5. Open **Analytics** to calculate totals and top groups for the current query.
6. Double-click a row to open its full details.
7. Right-click a row for quick copy and quick filter actions.
8. Export the current result set to CSV or XLSX.

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

- Windows 10/11
- Rust stable with the MSVC toolchain

Commands:

```powershell
cargo test
cargo build --release
```

Release binaries are created in:

```text
target\release\BaseSearch.exe
target\release\base-search-cli.exe
```

## Architecture

- **Rust** for the application core and Windows executables.
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

## License

Base Search is released under the MIT License. You can use, copy, modify, and
redistribute the application and source code, as long as the copyright notice
and license text are included.
