# WAV Metadata extraction

The scanner reads embedded Metadata chunks before and after the audio payload without decoding or rewriting source audio. The database and desktop expose both the source fields and a preferred source description. Library tags, notes, ratings, favorites and colors remain user annotations and are never assigned from embedded fields.

## Supported sources

| Container | Extracted data |
| --- | --- |
| BWF `bext` | Description, originator/reference, origination date/time, time reference, version, UMID and coding history |
| RIFF `LIST/INFO` | All text subchunks, including ICMT, INAM, IKEY, IART, IPRD, IGNR and ICOP |
| `iXML` | Direct BWFXML scene/take/tape/flags plus arbitrary nested USER/vendor fields and attributes |
| `axml` / `aXML` | XML text and attributes, including vendor extensions |
| `_PMX` / `XMP ` | XMP/RDF text, attribute properties and repeated array values |
| `id3 ` / `ID3 ` | ID3v2.2/2.3/2.4 text frames, custom TXXX fields and COMM comments; extended headers and unsynchronisation |
| `SMED` | Legacy Soundminer Blowfish/ECB wrapper containing UTF-16 XML; library/vendor text fields, excluding embedded pictures and Waveform caches |

XML uses a real parser for namespaces, entities, numeric character references and CDATA. UTF-8 and UTF-16 BOMs and declared XML encodings are decoded. INFO and fixed BWF text use valid UTF-8 first, otherwise Windows-1252; ambiguous unmarked legacy code pages cannot be inferred reliably. ID3 supports its Latin-1, UTF-16 and UTF-8 text encodings.

`attributes.wav_metadata` owns an extraction version marker (currently 3), bounded chunk inventory, source groups (`riff_info`, `ixml`, `axml`, `xmp`, `id3`, `smed`), normalized `fields` and optional `warnings`. XML keys use expanded namespace names and paths; distinct repeated values become arrays. Unknown vendor text remains available under its source path. Normalized fields include description, title, category/subcategory, CatID, keywords, library, designer, artist, publisher, copyright, microphone and location.

Preferred descriptions use SMED, iXML, XMP, aXML, INFO and ID3 in that order, then BWF as fallback. Conflicting values remain in the source groups and summary arrays; the inspector shows the first preferred value. `bext_description` retains its original BWF meaning.

## Rescanning and search

Playback and Waveform decoding use a separate lightweight header path that seeks past embedded Metadata. Audition does not repeatedly decrypt SMED artwork or parse large XML/ID3 chunks; full extraction runs only when scanning or explicitly probing Metadata.

An ordinary incremental scan checks both the file fingerprint and extraction version. A pre-fix record is re-probed once even if its size and timestamp are unchanged; later scans resume skipping unchanged records. There is no implicit scan on application startup. Select the existing library folder and scan to refresh it after upgrading.

Successful rescans replace the owned source group, clear removed fields and old search terms, and preserve unrelated extension Metadata and all user annotations. A failed WAV probe leaves the existing record unchanged. The schema remains v3: the existing `bwf_description_tokenized` derived column contains BWF plus extracted source text. FTS5 and Chinese segmentation therefore find embedded descriptions, categories, keywords and custom fields without copying them into user tags. Search rebuilds use the same derivation. The tag filter continues to filter user tags.

## Limits and diagnostics

Actual source values are preserved. For example, a supplied commercial-library sample stores `#BEZUG!` in Keywords and TrackTitle and an empty/NSNull Designer. These are source contents, not failed extraction; FXName and XMP title still provide its meaningful title. Manufacturer and Publisher remain distinct fields.

Metadata is limited to 2 MiB per public chunk, 16 MiB per SMED chunk and 32 MiB per file, XML to 16,384 nodes and 32 levels, with bounded field paths and search text. SMED has a larger bound because modern sound libraries include several MiB of UTF-16 base64 artwork. XML DTDs/external entities are rejected. Binary ID3 frames are skipped; compressed/encrypted ID3 text is reported as unsupported. Malformed optional containers generate warnings while valid audio and other successfully parsed Metadata remain accessible. Structurally truncated WAV chunks still fail the probe, preserving the previous database record.

Soundminer `SMED` is private and version-dependent. This reader supports the observed legacy wrapper: a big-endian decoded length followed by Blowfish/ECB blocks with the published compatibility key, then BOM-marked XML. It validates lengths before decryption and applies the same XML limits. Unsupported layouts or malformed XML produce a diagnostic; support for every Soundminer version is not claimed. The cipher exists solely for reading this legacy format, never for application security.

The 34 local P-51 Mustang WAV fixtures contain `fmt `, `bext`, `data`, `iXML` and `SMED`; their iXML contains version declarations rather than the professional library fields. The SMED reader restores Category (Vehicles), SubCategory (Aircraft), Microphone, Library and Manufacturer (BOOM Library), plus all other text fields. Embedded Picture data is excluded from the database and search index. The opt-in `real_boom_smed_metadata_fixtures` test reads these local WAVs; the release runtime regression verifies their fields and FTS5 recall in an isolated library.

On 2026-10-08 the release CLI scanned an anonymized commercial WAV sample on a network share. Every file exposed SMED, iXML, INFO, ID3 and XMP text with zero scan errors or Metadata warnings; Description, Category, SubCategory, CatID, Library, Manufacturer and VendorCategory were present in every record. The library-name search returned every sample, and a second incremental scan skipped every unchanged file. Source sizes and modification times were unchanged. The measured full scan took 45.16 seconds including CLI startup while other validation ran; this is a network-path scan with multi-MiB embedded artwork, not an SSD throughput or million-record search benchmark. Source paths, record counts and capacity are omitted from the public report. See validation data.

References: [Soundminer Metadata formats](https://info.soundminer.com/docs/about-metadata), [Soundminer XMP mappings](https://info.soundminer.com/docs/xmp), [original SMED format analysis](https://reverseengineering.stackexchange.com/questions/26241/extract-closed-format-metadata-from-audio-file), [RustCrypto Blowfish](https://docs.rs/blowfish/0.9.1/blowfish/), [ID3v2.3](https://id3.org/id3v2.3.0), [ID3v2.4 structure](https://id3.org/id3v2.4.0-structure).
