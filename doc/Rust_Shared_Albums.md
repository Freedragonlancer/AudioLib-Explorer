# Shared albums and Metadata storage (schema 8)

The current writable format is `ale-s8-cld1-t3-m2-p1-a1-z1`. It adds the
`album_entities: 1` and `metadata_storage: 1` format features. Upgrade clients and
servers together: older binaries that support schemas only through 7 cannot open
a native schema-8 snapshot. HTTP API v1 remains additive.

## Album ownership

An album belongs to a Metadata library or a folder, rather than a selected WAV.
The trimmed, case-sensitive WAV Metadata `library` value identifies a shared
Metadata album. With no such value, the registered scan root's first child folder
identifies the folder album; files directly under a root use that root. Records
with no registered folder share an unassigned folder entity. Metadata and folder
albums are different kinds even when their display names match. Directories can
contain multiple Metadata libraries, so directory names never replace a declared
Metadata library.

`audio_files.album_id` references `albums`; the old duplicated `album` text column
is retired. Owned DTOs retain `AudioFile.album` as the declared Metadata value and
add `album_id` / `album_entity`. Search accepts either the legacy album name or a
positive album ID, never both. Sidebar facets carry their ID and matching count
in one provider query and retain the current search and other filters.

Album IDs are scoped by a persistent random namespace. An allocator high-water
mark prevents ID reuse after removal and restore. Root relinking preserves the
folder entity; a file moved into another album adopts that album. Reparenting
registered roots recomputes affected fallback memberships. Backup/restore retains
identity; import as a new library creates a fresh namespace. Restoring updates
the provider's cached identity before readers and UI generations are renewed.

Personal search tasks bind album filters to that namespace and semantic owner.
Native-server upgrades resolve all open and saved filters before executing a
numeric ID. Missing owners remain visibly unavailable; explicit reselection can
choose a new album even when it reuses an old numeric ID in another namespace.
Workspace format 2 prevents older clients from ignoring these conditions; see
[search-task recovery](Rust_Search_Tasks.md).

Pictures remain outside SQLite, FTS5 and snapshots. A bounded, sorted set of up to
64 present members supplies the first valid embedded picture for the whole
album. Manual cover selection, hiding and restore-to-embedded belong to that
shared entity. See [artwork](Rust_Artwork.md) for caching and old-choice migration.

## Lossless Metadata storage

`metadata_json` contains either original UTF-8 TEXT or a versioned zlib BLOB.
Values smaller than 256 bytes, values above 64 MiB, and values saving 32 bytes or
less including the header stay TEXT. Eligible values use zlib level 6 with
the `ALEMD` versioned header and original byte length. Decoding checks the header,
length, stream end, checksum, trailing bytes and UTF-8. Corrupt data is reported;
it is never replaced by an empty Metadata object.

Compression preserves the exact JSON bytes, including unknown fields, whitespace
and key order. DataProvider, DTOs, API replies and JSONL/CSV export continue to
return ordinary decoded Metadata. All application reads and writes use the
codec. SQL code maintained by this project registers
`audiolib_metadata_json(metadata_json)` before using SQLite JSON functions.
External tools must use the application API/export or decode the versioned value;
they cannot assume that every physical `metadata_json` value is SQLite TEXT.

Unknown triggers, views, generated columns, CHECK constraints or expression
indexes that might depend on plaintext Metadata, STRICT TEXT declarations, and
Metadata uniqueness/collation constraints cause the entire library to retain TEXT
storage. An extension added to a compressed library requires an
owned maintenance repair before new writes; the application does not silently
break that extension. Unknown business tables and implicit rowids are retained.

FTS5 indexes searchable fields independently of Metadata compression. Search
result hydration decodes only returned rows. Compression therefore adds decode
work to result materialization, not to every indexed row during a search. The
performance target remains **P95 latency <500ms for 1M+ records**; measured
conditions and results must be reported separately from this target.

### Pre-publication measurements

A read-only rehearsal of a 909,342-record library reduced the SQLite file from
10,611,302,400 to 5,400,199,168 bytes (49.11%). All original Metadata UTF-8 bytes
matched after decoding, including whitespace and unknown fields. The 6,910
shared entities covered every record without orphan memberships. Rollback and
staging files are additional storage, outside this final-file comparison.

On a Ryzen 7 9800X3D with a Windows local SSD, warm ABBA runs of the same 100-query
mix measured result-page P95 at 99–101 ms before and 109–113 ms after migration.
A separate 1M-record synthetic fixture measured 117.7 ms for result pages and
212.2 ms including directory facets. That fixture has one fallback album; the
real-library rehearsal supplies the broader album cardinality. Provider opening,
cold-cache startup and complete UI latency are separate measurements.

Physical row compression does not guarantee a smaller transport archive. With
the server's zstd level 3 and eight workers, the rehearsal snapshot grew from
950,903,523 to 2,033,256,351 bytes. Per-row zlib hides redundancy that outer zstd
previously shared across records. The current implementation trades a smaller
installed database for a larger initial/forced snapshot transfer; it does not
claim faster synchronization. A text-oriented wire projection would need separate
owned-copy conversion, cancellation and publication proofs on both peers.

## Migration and recovery

Recognized schemas 3–7 are read-only sources until migration. Migration reviews
the source format and extension dependencies, creates an online rollback backup,
copies into an owned candidate, backfills albums, encodes eligible Metadata and
rebuilds FTS5. Business digests compare decoded original Metadata bytes; new album
bindings are then included in the target digest. Full format and data validation
precede publication. An unsupported dependency or malformed source fails without
replacing the original library.

Album backfill creates a uniquely named covering index for folder ancestry in
the owned transaction before processing members, then drops only that index.
This avoids a full closure scan per WAV before permanent indexes are installed.
Cancellation rolls the transaction back; unknown schema objects stay intact.

Copying into the new candidate physically repacks storage. The final phase merges
FTS segments, but does not run an unrestricted VACUUM that could renumber implicit
rowids in unknown extension tables. Backup and staging files require additional
free space and are retained for recovery. Removing them is a separate recovery
decision, never an installation cleanup step.

Remote migration changes only an owned local copy and retains its source-format
descriptor. Old-source projections retain a local namespace and member IDs across
revisions. Native schema-8 snapshots use the server's namespace. Namespace and
library headers prevent the same numeric album ID from selecting a cover from a
different library or imported database.
