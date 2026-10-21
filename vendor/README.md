# Pinned SQLite engine

`libsqlite3-sys/` is the crates.io `libsqlite3-sys` 0.38.2 source, retaining its
upstream licenses, build configuration and Rust bindings. Only
`sqlite3/sqlite3.c`, `sqlite3/sqlite3.h` and `sqlite3/sqlite3ext.h` are replaced
with SQLite's official 3.54.1 amalgamation. The application uses bundled SQLite;
SQLCipher is not an enabled application feature.

- Source: https://www.sqlite.org/2026/sqlite-amalgamation-3540100.zip
- Archive SHA3-256:
  `92281c86e2957f9b9305890f9ac04204caa7b677cc868dc326d942689df28e30`
- SQLite source ID:
  `2026-10-16 14:45:20 555e07ef33f69a0957d24d8e555b702464a131d6f1607f4c3c1e4551e8e6ca76`
- Required correction:
  https://github.com/sqlite/sqlite/commit/b0d5cbd
  (contentless-delete row/column totals after deletion).

Cargo's workspace patch selects this dependency for the client, CLI and server.
Builds do not download or substitute an implicit SQLite engine. Runtime guards
and update/delete BM25 regression tests must accompany upgrades. When upstream
ships the corrected engine, remove this patch only after running those tests
and the full migration matrix against the actual linked replacement.
