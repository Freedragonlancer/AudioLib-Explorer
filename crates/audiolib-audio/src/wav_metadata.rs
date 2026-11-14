// crates/audiolib-audio/src/wav_metadata.rs
//! Bounded, read-only extraction of public WAV Metadata containers.
use encoding_rs::{Encoding, GBK, SHIFT_JIS, UTF_16BE, UTF_16LE, WINDOWS_1252};
use roxmltree::{Document, Node, ParsingOptions};
use serde_json::{Map, Value, json};

/// Increment when extraction changes so an ordinary scan refreshes old records.
pub const WAV_METADATA_VERSION: i64 = 5;
pub(crate) const MAX_CHUNK_BYTES: u64 = 2 * 1024 * 1024;
// Modern SMED includes UTF-16 base64 artwork (the supplied BOOM files use ~4 MiB).
pub(crate) const MAX_SMED_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_METADATA_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FIELDS: usize = 16_384;
const MAX_XML_DEPTH: usize = 32;
const MAX_FIELD_PATH_BYTES: usize = 4096;
/// Bounded decoded cover payload; larger pictures are ignored rather than retained.
pub(crate) const MAX_ARTWORK_BYTES: usize = 8 * 1024 * 1024;
// An ID3 tag can contain one bounded image plus text/frame overhead. Ordinary
// Metadata extraction still skips binary frames and caps copied text separately.
pub(crate) const MAX_ID3_BYTES: u64 = MAX_ARTWORK_BYTES as u64 + MAX_CHUNK_BYTES;

/// Cover artwork embedded in the WAV itself (ID3 or SMED), kept separate from Metadata JSON
/// so a multi-megabyte image never reaches SQLite or FTS5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artwork {
    /// Original JPEG/PNG bytes, never re-encoded by the extractor.
    pub bytes: Vec<u8>,
    /// MIME type detected from the image magic, never trusted from a tag declaration.
    pub mime: String,
}

/// Decode NUL-terminated INFO, BWF, and AIFF text.
///
/// Valid UTF-8 wins, so ASCII and well-formed Unicode stay intact. Other bytes
/// are GBK or Shift-JIS when they form a plausible CJK or kana run, and
/// Windows-1252 otherwise. That keeps Latin-1 words and illegal bytes on the
/// previous fallback instead of inventing a CJK reading.
pub(crate) fn text(bytes: &[u8]) -> String {
    let bytes = bytes.split(|b| *b == 0).next().unwrap_or_default();
    decode_unmarked_text(bytes).trim().to_string()
}

fn decode_unmarked_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(value) = std::str::from_utf8(bytes) {
        return value.to_string();
    }
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] < 0x80 {
            let start = index;
            index += 1;
            while index < bytes.len() && bytes[index] < 0x80 {
                index += 1;
            }
            out.push_str(std::str::from_utf8(&bytes[start..index]).unwrap());
            continue;
        }
        let end = legacy_span_end(bytes, index);
        let run = &bytes[index..end];
        if let Some(decoded) = decode_legacy_run(run) {
            out.push_str(&decoded);
        } else {
            out.push_str(&windows_1252(run));
        }
        index = end;
    }
    out
}

fn windows_1252(bytes: &[u8]) -> String {
    WINDOWS_1252
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

fn legacy_span_end(bytes: &[u8], start: usize) -> usize {
    let high_end = high_byte_end(bytes, start);
    let sjis_end = sjis_greedy_end(bytes, start);
    if sjis_end > high_end && sjis_extension_ok(&bytes[start..sjis_end]) {
        sjis_end
    } else if high_end > start {
        high_end
    } else {
        start + 1
    }
}

fn high_byte_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < bytes.len() && bytes[index] >= 0x80 {
        index += 1;
    }
    index
}

/// Windows-1252 punctuation bytes that are also Shift-JIS leads.
///
/// An en dash or quote must not swallow the following ASCII letter (`it’s`,
/// `a–b`). The same lead with a non-letter trail is a kanji (`本` is
/// `0x96 '{'`) and can be consumed.
fn refuses_letter_trail(lead: u8) -> bool {
    matches!(
        lead,
        0x82 | 0x84
            | 0x85
            | 0x86
            | 0x87
            | 0x91
            | 0x92
            | 0x93
            | 0x94
            | 0x95
            | 0x96
            | 0x97
            | 0x99
            | 0x9B
    )
}

const fn is_sjis_lead(byte: u8) -> bool {
    matches!(byte, 0x81..=0x9F | 0xE0..=0xFC)
}

const fn is_sjis_trail(byte: u8) -> bool {
    matches!(byte, 0x40..=0x7E | 0x80..=0xFC)
}

fn sjis_greedy_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < bytes.len() {
        let lead = bytes[index];
        if (0xA1..=0xDF).contains(&lead) {
            index += 1;
            continue;
        }
        if is_sjis_lead(lead) && index + 1 < bytes.len() && is_sjis_trail(bytes[index + 1]) {
            let trail = bytes[index + 1];
            if trail < 0x80 && refuses_letter_trail(lead) && trail.is_ascii_alphabetic() {
                break;
            }
            index += 2;
            continue;
        }
        break;
    }
    index
}

fn sjis_extension_ok(run: &[u8]) -> bool {
    if !run.iter().any(|byte| *byte < 0x80) {
        return false;
    }
    let Some(text) = strict_text(SHIFT_JIS, run) else {
        return false;
    };
    let kana = count_chars(&text, is_fullwidth_kana);
    let kanji = count_chars(&text, is_cjk_ideograph);
    if kana >= 2 || kanji >= 2 {
        return true;
    }
    // One pair whose trail is not an ASCII letter is not `it’s` or `Œuvre`.
    kana + kanji >= 1 && !run.iter().any(|byte| byte.is_ascii_alphabetic())
}

struct LegacyCandidate {
    text: String,
    gb2312: u32,
    sjis_low: u32,
    kana: u32,
    halfwidth: u32,
}

fn decode_legacy_run(run: &[u8]) -> Option<String> {
    if run.len() < 2 || prefer_windows_1252_latin(run) {
        return None;
    }
    choose_legacy(gbk_candidate(run), sjis_candidate(run))
}

fn choose_legacy(gbk: Option<LegacyCandidate>, sjis: Option<LegacyCandidate>) -> Option<String> {
    match (gbk, sjis) {
        (None, None) => None,
        (Some(gbk), None) => Some(gbk.text),
        (None, Some(sjis)) => Some(sjis.text),
        (Some(gbk), Some(sjis)) => Some(if prefer_sjis(&gbk, &sjis) {
            sjis.text
        } else {
            gbk.text
        }),
    }
}

fn prefer_sjis(gbk: &LegacyCandidate, sjis: &LegacyCandidate) -> bool {
    if sjis.kana > 0 && (gbk.gb2312 == 0 || sjis.kana >= gbk.gb2312) {
        return true;
    }
    if sjis.sjis_low > gbk.gb2312 {
        return true;
    }
    if gbk.gb2312 > sjis.sjis_low {
        return false;
    }
    // Shared E0–F7 leads are GB2312 as well as Shift-JIS. Without kana or an
    // 81–9F lead, keep the Chinese reading.
    gbk.gb2312 == 0 && (sjis.kana > 0 || sjis.sjis_low > 0 || sjis.halfwidth >= 4)
}

fn gbk_candidate(run: &[u8]) -> Option<LegacyCandidate> {
    if run.iter().any(|byte| *byte < 0x80) || run.iter().all(|byte| (0x80..0xC0).contains(byte)) {
        return None;
    }
    let text = strict_text(GBK, run)?;
    if count_chars(&text, is_cjk_ideograph) == 0 {
        return None;
    }
    Some(LegacyCandidate {
        text,
        gb2312: count_gb2312(run),
        sjis_low: 0,
        kana: 0,
        halfwidth: 0,
    })
}

fn sjis_candidate(run: &[u8]) -> Option<LegacyCandidate> {
    let text = strict_text(SHIFT_JIS, run)?;
    let kana = count_chars(&text, is_fullwidth_kana);
    let kanji = count_chars(&text, is_cjk_ideograph);
    let halfwidth = count_chars(&text, is_halfwidth_kana);
    let sjis_low = count_sjis_low(run);
    if kana == 0 && kanji == 0 && (halfwidth < 4 || !run.iter().any(|byte| *byte >= 0xC0)) {
        return None;
    }
    Some(LegacyCandidate {
        text,
        gb2312: 0,
        sjis_low,
        kana,
        halfwidth,
    })
}

fn strict_text(encoding: &'static Encoding, bytes: &[u8]) -> Option<String> {
    let text = encoding
        .decode_without_bom_handling_and_without_replacement(bytes)?
        .into_owned();
    if text
        .chars()
        .any(|ch| ch == '\u{FFFD}' || ('\u{E000}'..='\u{F8FF}').contains(&ch))
    {
        return None;
    }
    Some(text)
}

fn count_chars(text: &str, kind: fn(char) -> bool) -> u32 {
    text.chars().filter(|ch| kind(*ch)).count() as u32
}

fn count_gb2312(run: &[u8]) -> u32 {
    let mut index = 0;
    let mut count = 0;
    while index < run.len() {
        let lead = run[index];
        if lead < 0x81 || index + 1 >= run.len() {
            index += 1;
            continue;
        }
        let trail = run[index + 1];
        let valid_trail = (0x40..=0x7E).contains(&trail) || (0x80..=0xFE).contains(&trail);
        if (0x81..=0xFE).contains(&lead) && valid_trail {
            if (0xB0..=0xF7).contains(&lead) && (0xA1..=0xFE).contains(&trail) {
                count += 1;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    count
}

fn count_sjis_low(run: &[u8]) -> u32 {
    let mut index = 0;
    let mut count = 0;
    while index < run.len() {
        let lead = run[index];
        if (0xA1..=0xDF).contains(&lead) {
            index += 1;
            continue;
        }
        if is_sjis_lead(lead) && index + 1 < run.len() && is_sjis_trail(run[index + 1]) {
            if (0x81..=0x9F).contains(&lead) {
                count += 1;
            }
            index += 2;
        } else if lead < 0x80 {
            index += 1;
        } else {
            break;
        }
    }
    count
}

fn is_cjk_ideograph(ch: char) -> bool {
    matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

fn is_fullwidth_kana(ch: char) -> bool {
    matches!(ch, '\u{3040}'..='\u{30FF}')
}

fn is_halfwidth_kana(ch: char) -> bool {
    matches!(ch, '\u{FF61}'..='\u{FF9F}')
}

/// Accented European bytes that are also legal CJK pairs.
///
/// A run of Windows-1252 letters stays Latin when lowercase letters predominate
/// (Größe, äö) or it is one of the adjacent Scandinavian pairs. Mixed and
/// uppercase pairs in the GB2312 range are read as hanzi: those two-byte
/// sequences are how unmarked Chinese metadata is stored.
fn prefer_windows_1252_latin(run: &[u8]) -> bool {
    if run.is_empty() || !run.iter().copied().all(is_prose_latin_byte) {
        return false;
    }
    if run.len() == 2 && run.contains(&0xDF) {
        return true;
    }
    let lower = run
        .iter()
        .copied()
        .filter(|byte| is_prose_lower_byte(*byte))
        .count();
    let upper = run
        .iter()
        .copied()
        .filter(|byte| is_prose_upper_byte(*byte))
        .count();
    if lower > upper {
        return true;
    }
    if run.len() == 2 {
        let text = windows_1252(run);
        return ["ÄÖ", "ÖÄ", "ÅÄ", "ÄÅ", "ÆØ", "ØÆ", "ÜÖ", "ÖÜ", "ÅÖ", "ÖÅ"]
            .iter()
            .any(|pair| *pair == text);
    }
    false
}

const fn is_prose_latin_byte(byte: u8) -> bool {
    is_prose_lower_byte(byte) || is_prose_upper_byte(byte)
}

const fn is_prose_upper_byte(byte: u8) -> bool {
    matches!(
        byte,
        0x8A | 0x8C | 0x8E | 0x9F | 0xC0..=0xD6 | 0xD8..=0xDE
    )
}

const fn is_prose_lower_byte(byte: u8) -> bool {
    matches!(
        byte,
        0x9A | 0x9C | 0x9E | 0xDF | 0xE0..=0xF6 | 0xF8..=0xFF
    )
}

fn xml_text(bytes: &[u8]) -> String {
    let encoding = Encoding::for_bom(bytes)
        .map(|(encoding, _)| encoding)
        .or_else(|| {
            if bytes.starts_with(b"<\0") {
                Some(UTF_16LE)
            } else if bytes.starts_with(b"\0<") {
                Some(UTF_16BE)
            } else {
                // The declaration is ASCII even when the following XML text uses a
                // legacy encoding. Do not require that source text to be UTF-8.
                bytes.strip_prefix(b"<?xml")?;
                let end = bytes.windows(2).position(|pair| pair == b"?>")?;
                let declaration = std::str::from_utf8(&bytes[..end]).ok()?;
                let label = declaration
                    .split_once("encoding")?
                    .1
                    .trim_start()
                    .strip_prefix('=')?
                    .trim_start();
                let quote = label.chars().next()?;
                let label = label.strip_prefix(quote)?.split(quote).next()?;
                Encoding::for_label(label.as_bytes())
            }
        });
    let decoded = match encoding {
        Some(encoding) => encoding.decode(bytes).0,
        None => String::from_utf8_lossy(bytes),
    };
    decoded.trim_matches(['\0', '\u{feff}']).trim().to_string()
}

pub(crate) fn append(map: &mut Map<String, Value>, key: &str, value: &str) {
    let value = value.trim_matches(['\0', '\u{feff}']).trim();
    if value.is_empty() || map.len() >= MAX_FIELDS {
        return;
    }
    match map.get_mut(key) {
        None => {
            map.insert(key.into(), json!(value));
        }
        Some(Value::Array(values)) => {
            if values.len() < MAX_FIELDS && !values.iter().any(|v| v.as_str() == Some(value)) {
                values.push(json!(value));
            }
        }
        Some(previous) if previous.as_str() != Some(value) => {
            *previous = json!([previous.clone(), value]);
        }
        _ => {}
    }
}

#[derive(Default)]
pub(crate) struct EmbeddedMetadata {
    groups: Map<String, Value>,
    chunks: Vec<Value>,
    warnings: Vec<String>,
    artwork: Option<Artwork>,
    artwork_priority: Option<ArtworkPriority>,
}

// A valid ID3 picture overrides SMED regardless of RIFF chunk order. Within ID3,
// prefer an explicitly tagged front cover, retaining the first equal-priority image.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum ArtworkPriority {
    Smed,
    Id3,
    Id3Front,
}

impl EmbeddedMetadata {
    fn accepts_artwork(&self, priority: ArtworkPriority) -> bool {
        self.artwork_priority
            .is_none_or(|previous| previous < priority)
    }

    /// Retain a bounded JPEG/PNG only when it improves the source/type priority.
    fn set_artwork(&mut self, bytes: Vec<u8>, priority: ArtworkPriority) -> bool {
        if !self.accepts_artwork(priority) || bytes.is_empty() || bytes.len() > MAX_ARTWORK_BYTES {
            return false;
        }
        let Some(mime) = image_mime(&bytes) else {
            return false;
        };
        self.artwork = Some(Artwork {
            bytes,
            mime: mime.to_owned(),
        });
        self.artwork_priority = Some(priority);
        true
    }

    pub(crate) fn take_artwork(&mut self) -> Option<Artwork> {
        self.artwork.take()
    }
}

/// The image consumers support bounded JPEG/PNG; a MIME label alone is insufficient.
fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else {
        None
    }
}

impl EmbeddedMetadata {
    pub(crate) fn smed(
        &mut self,
        bytes: &[u8],
        attrs: &mut Map<String, Value>,
        include_artwork: bool,
    ) {
        use blowfish::cipher::{Block, BlockDecrypt, KeyInit};
        if bytes.len() < 12 || !(bytes.len() - 4).is_multiple_of(8) {
            self.warning("SMED: unsupported block layout");
            return;
        }
        let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        if size == 0 || size > bytes.len() - 4 || bytes.len() - 4 - size >= 8 {
            self.warning("SMED: invalid decoded length");
            return;
        }
        // Legacy Soundminer compatibility only, never used to protect application data.
        // Public format analysis: https://reverseengineering.stackexchange.com/q/26241
        let cipher: blowfish::Blowfish =
            blowfish::Blowfish::new_from_slice(b"u7w58he4746").unwrap();
        let mut xml = bytes[4..].to_vec();
        for bytes in xml.chunks_exact_mut(8) {
            let mut block = Block::<blowfish::Blowfish>::default();
            block.copy_from_slice(bytes);
            cipher.decrypt_block(&mut block);
            bytes.copy_from_slice(&block);
        }
        xml.truncate(size);
        self.xml_with_artwork("smed", &xml, attrs, include_artwork);
    }

    pub(crate) fn chunk(&mut self, id: &[u8], size: u64) {
        if self.chunks.len() < 256 {
            self.chunks
                .push(json!({"id": String::from_utf8_lossy(id), "bytes": size}));
        }
    }

    pub(crate) fn warning(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.warnings.len() < 32 && !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    fn merge(&mut self, name: &str, values: Map<String, Value>) {
        let group = self
            .groups
            .entry(name)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap();
        for (key, value) in values {
            match value {
                Value::String(value) => append(group, &key, &value),
                Value::Array(values) => {
                    for value in values {
                        if let Some(value) = value.as_str() {
                            append(group, &key, value);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn info(&mut self, bytes: &[u8]) {
        if !bytes.starts_with(b"INFO") {
            return;
        }
        let mut fields = Map::new();
        let mut rest = &bytes[4..];
        for _ in 0..MAX_FIELDS {
            if rest.is_empty() {
                break;
            }
            if rest.len() < 8 {
                self.warning("LIST/INFO: truncated field header");
                break;
            }
            let size = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
            let Some(data) = rest.get(8..8 + size) else {
                self.warning("LIST/INFO: truncated field value");
                break;
            };
            append(
                &mut fields,
                &String::from_utf8_lossy(&rest[..4]),
                &text(data),
            );
            rest = rest.get(8 + size + (size & 1)..).unwrap_or_default();
        }
        self.merge("riff_info", fields);
    }

    pub(crate) fn xml(&mut self, group: &str, bytes: &[u8], attrs: &mut Map<String, Value>) {
        self.xml_with_artwork(group, bytes, attrs, false);
    }

    fn xml_with_artwork(
        &mut self,
        group: &str,
        bytes: &[u8],
        attrs: &mut Map<String, Value>,
        include_artwork: bool,
    ) {
        let xml = xml_text(bytes);
        if group == "ixml" {
            attrs.insert("ixml_raw".into(), json!(xml));
        }
        let document = match Document::parse_with_options(
            &xml,
            ParsingOptions {
                allow_dtd: false,
                nodes_limit: MAX_FIELDS as u32,
                ..Default::default()
            },
        ) {
            Ok(document) => document,
            Err(error) => {
                self.warning(format!("{group}: {error}"));
                return;
            }
        };
        // Reuse the decrypted text and bounded DOM. Artwork never enters fields,
        // ixml_raw, or the resulting Metadata JSON, and scans skip this work.
        if group == "smed" && include_artwork {
            self.read_smed_artwork(&document);
        }
        let mut fields = Map::new();
        for node in document.descendants().filter(Node::is_element) {
            // Apply the depth bound before any work that walks ancestor chains.
            let ancestors: Vec<_> = node
                .ancestors()
                .filter(Node::is_element)
                .take(MAX_XML_DEPTH + 1)
                .collect();
            if ancestors.len() > MAX_XML_DEPTH {
                self.warning(format!("{group}: nesting exceeds 32 levels"));
                continue;
            }
            // Namespace URIs can be much longer than tag names. Check each part
            // before copying it so a shared large URI cannot inflate every path.
            let path = ancestors
                .iter()
                .rev()
                .try_fold(String::new(), |mut path, n| {
                    if !path.is_empty() {
                        path.push('/');
                    }
                    append_xml_name(&mut path, n.tag_name().namespace(), n.tag_name().name())
                        .then_some(path)
                });
            let Some(path) = path else {
                self.warning(format!("{group}: field path exceeds 4096 bytes"));
                continue;
            };
            // SMED embeds base64 artwork/caches alongside searchable library fields.
            // Do not duplicate those binary blobs into SQLite or FTS5.
            if group == "smed"
                && ancestors.iter().any(|parent| {
                    parent
                        .attribute("type")
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("NSData"))
                        || [
                            "picture",
                            "artwork",
                            "waveform",
                            "waveformdata",
                            "waveformcache",
                        ]
                        .iter()
                        .any(|name| parent.tag_name().name().eq_ignore_ascii_case(name))
                })
            {
                continue;
            }
            let value: String = node
                .children()
                .filter(Node::is_text)
                .filter_map(|n| n.text())
                .collect();
            append(&mut fields, &path, &value);
            for attribute in node.attributes() {
                let mut key = path.clone();
                key.push_str("/@");
                if append_xml_name(&mut key, attribute.namespace(), attribute.name()) {
                    append(&mut fields, &key, attribute.value());
                } else {
                    self.warning(format!("{group}: field path exceeds 4096 bytes"));
                }
            }
        }
        if group == "ixml" {
            // Read only direct BWFXML children; track/user tags with the same names are unrelated.
            for node in document.root_element().children().filter(Node::is_element) {
                let value: String = node
                    .children()
                    .filter(Node::is_text)
                    .filter_map(|n| n.text())
                    .collect();
                let value = value.trim();
                let key = match node.tag_name().name() {
                    "SCENE" => "ixml_scene",
                    "TAKE" => "ixml_take",
                    "TAPE" => "ixml_tape",
                    "CIRCLED" => "ixml_circled",
                    "WILD_TRACK" => "ixml_wild_track",
                    "NO_GOOD" => "ixml_no_good",
                    _ => continue,
                };
                if ["ixml_scene", "ixml_take", "ixml_tape"].contains(&key) {
                    attrs.insert(key.into(), json!(value));
                } else if !value.is_empty() {
                    attrs.insert(
                        key.into(),
                        json!(matches!(
                            value.to_ascii_uppercase().as_str(),
                            "TRUE" | "1" | "YES"
                        )),
                    );
                }
            }
        }
        self.merge(group, fields);
    }

    pub(crate) fn finish(mut self, attrs: &mut Map<String, Value>) {
        let mut fields = Map::new();
        // Rich description sources precede the fixed 256-byte BWF description.
        // Keep every original value under its source path even when summaries overlap.
        for group in ["smed", "ixml", "xmp", "axml", "riff_info", "id3"] {
            if let Some(values) = self.groups.get(group).and_then(Value::as_object) {
                let mut values: Vec<_> = values.iter().collect();
                // Copyright comments/notes must not hide the actual description, and
                // SMED's full Description must precede its truncated BWDescription copy.
                values.sort_by_key(|(path, _)| match field_name(path).as_str() {
                    "bwdescription" => 1,
                    "note" | "comment" | "comments" => 2,
                    _ => 0,
                });
                for (path, value) in values {
                    if let Some(key) = canonical_field(group, path) {
                        match value {
                            Value::String(value) => append(&mut fields, key, value),
                            Value::Array(values) => {
                                for value in values {
                                    if let Some(value) = value.as_str() {
                                        append(&mut fields, key, value);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if !fields.contains_key("description")
            && let Some(value) = attrs.get("bwf_description").and_then(Value::as_str)
        {
            append(&mut fields, "description", value);
        }
        self.groups.insert("fields".into(), Value::Object(fields));
        self.groups
            .insert("version".into(), json!(WAV_METADATA_VERSION));
        self.groups.insert("chunks".into(), json!(self.chunks));
        if !self.warnings.is_empty() {
            self.groups.insert("warnings".into(), json!(self.warnings));
        }
        attrs.insert("wav_metadata".into(), Value::Object(self.groups));
    }
}

fn append_xml_name(path: &mut String, namespace: Option<&str>, name: &str) -> bool {
    // Expanded names preserve unknown vendor fields and prevent prefix collisions.
    let namespace_bytes = namespace.map_or(0, |namespace| namespace.len() + 2);
    if path.len() + namespace_bytes + name.len() > MAX_FIELD_PATH_BYTES {
        return false;
    }
    if let Some(namespace) = namespace {
        path.push('{');
        path.push_str(namespace);
        path.push('}');
    }
    path.push_str(name);
    true
}

fn field_name(path: &str) -> String {
    // Namespace URIs may contain '/'. Strip expanded namespaces before splitting paths.
    let mut plain = String::new();
    let mut namespace = false;
    for ch in path.chars() {
        match ch {
            '{' => namespace = true,
            '}' => namespace = false,
            _ if !namespace => plain.push(ch),
            _ => {}
        }
    }
    let name = plain
        .split('/')
        .rev()
        .find(|name| !["li", "Bag", "Seq", "Alt"].contains(name))
        .unwrap_or_default()
        .trim_start_matches('@');
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn canonical_field(group: &str, path: &str) -> Option<&'static str> {
    if group == "id3" && (path.starts_with("COMM/") || path.starts_with("COM/")) {
        return Some("description");
    }
    let normalized = field_name(path);
    Some(match (group, normalized.as_str()) {
        ("riff_info", "icmt" | "isbj") | ("id3", "comm" | "com") => "description",
        ("riff_info", "inam") | ("id3", "tit2" | "tt2") => "title",
        ("riff_info", "ikey") => "keywords",
        ("riff_info", "iart") | ("id3", "tpe1" | "tp1") => "artist",
        ("riff_info", "iprd") | ("id3", "talb" | "tal") => "library",
        ("riff_info", "ignr") | ("id3", "tcon" | "tco") => "category",
        ("riff_info", "icop") | ("id3", "tcop" | "tcr") => "copyright",
        (_, "description" | "bwdescription" | "note" | "comment" | "comments") => "description",
        (_, "title" | "tracktitle" | "fxname") => "title",
        (_, "category" | "genre") => "category",
        (_, "subcategory") => "subcategory",
        (_, "vendorcategory") => "vendor_category",
        (_, "catid" | "categoryid") => "cat_id",
        (_, "keywords" | "subject" | "tags") => "keywords",
        (_, "library" | "cdtitle" | "album") => "library",
        (_, "designer" | "sounddesigner") => "designer",
        (_, "artist" | "creator") => "artist",
        (_, "manufacturer") => "manufacturer",
        (_, "publisher" | "musicpublisher") => "publisher",
        (_, "copyright" | "rights") => "copyright",
        (_, "microphone" | "micmodel") => "microphone",
        (_, "location") => "location",
        _ => return None,
    })
}

#[path = "wav_id3.rs"]
mod id3;

#[path = "wav_artwork.rs"]
mod artwork;

#[cfg(test)]
#[path = "wav_metadata_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "wav_artwork_tests.rs"]
mod artwork_tests;
