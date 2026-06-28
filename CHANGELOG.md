# Changelog

All notable project changes are documented here.

## 1.4.1

- Optimized startup migration for existing large databases so compatible FTS
  indexes are reused instead of being rebuilt unnecessarily.
- Added database storage reporting to `base-search-cli stats`: main database
  file size, WAL size, SHM size, SQLite free pages, and total storage.
- Added `base-search-cli compact <db> [--vacuum]` for safe WAL truncation and
  optional SQLite `VACUUM` compaction without deleting records.
- Ignored local release package folders so zip artifacts do not get committed
  accidentally.
- Rebuilt the Windows distribution binaries for the 1.4.1 release.

## 1.4.0

- Added a flexible desktop Advanced Search builder with editable rule chips,
  all/any groups, exclusion rules and groups, nested groups, range filters,
  empty/not-empty checks, and extra-column conditions.
- Added a universal structured query model that keeps the current flat filters
  working while compiling advanced rules into parameterized SQLite queries.
- Added V2 saved and recent search serialization for advanced queries, with
  backwards-compatible decoding for legacy saved searches.
- Added a field catalog that combines known record fields, the virtual year
  field, and extra headers discovered from imported spreadsheets.
- Localized the new Advanced Search controls, operators, hints, and summaries
  across all 11 supported interface languages.
- Rebuilt the Windows distribution binaries for the 1.4.0 release.

## 1.3.0

- Preserved columns beyond the known schema with each imported row, included
  them in full-text search, and exposed them on record cards.
- Hardened local browser mode with stricter request parsing, per-session API
  tokens, and a fixed worker pool with reused database connections.

## 1.2.0

- Added the context-aware Questions menu for routing common business questions
  into the correct analytics view.
- Expanded customs header hints and glossary coverage.
- Added local browser mode, printable reports, compare mode, and company dossier
  polish.

## 1.1.1

- Added guided first-run scripts for macOS and Linux.
- Added 11 interface languages and CJK font fallback.
- Centralized more UI strings in the translation layer.

## 1.1

- Added cross-platform support for Windows, Linux, and macOS.
- Reworked Analytics into focused sub-tabs and added pivot, company dossier,
  price-undervaluation scan, and CI builds.

## 1.0

- Initial public release with Excel import, SQLite/FTS5 search, filters,
  analytics, duplicate protection, CSV/XLSX export, and light/dark themes.
