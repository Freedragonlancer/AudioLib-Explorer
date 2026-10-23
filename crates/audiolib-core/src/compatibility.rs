// crates/audiolib-core/src/compatibility.rs
//! Owned, conservative compatibility decisions shared by every application adapter.
//! Database observations must come from a pinned inspection, never from SemVer alone.
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Catalog version identifies capability semantics independently of application SemVer.
pub const CATALOG_REVISION: u32 = 1;
/// Plan B's completed business schema.
pub const TARGET_SCHEMA: u32 = 6;
/// Completed tokenizer encoding; 4 in an old live upgrade is a temporary marker.
pub const TOKENIZATION_REVISION: u32 = 3;
/// Format identifiers are exact sets, not a promise that all lower integers work.
pub const PLAN_B_FORMAT: &str = "ale-s6-cld1-t3-m2-p1";
/// Earlier unpublished Plan B retained the last playback timestamp; read-only source.
pub const PLAN_B_INITIAL_FORMAT: &str = "ale-s6-cld1-t3-m2-p0";
/// Snapshot protocol is separate from both schema and application versions.
pub const API_PROTOCOL: &str = "api/v1";
/// Minimum engine with corrected contentless-delete aggregate statistics.
pub const MINIMUM_SQLITE_VERSION: &str = "3.54.1";
/// Bounded JSON capability catalog supplied by a modern snapshot client.
pub const CLIENT_COMPATIBILITY_HEADER: &str = "x-audiolib-client-compatibility";
/// Server catalog attached to a modern snapshot response.
pub const SERVER_COMPATIBILITY_HEADER: &str = "x-audiolib-server-compatibility";
/// Exact accepted format ID list; never interpreted as an integer upper bound.
pub const ACCEPT_FORMATS_HEADER: &str = "x-audiolib-accept-snapshot-formats";
/// Completed format ID of the actual transmitted snapshot.
pub const SNAPSHOT_FORMAT_HEADER: &str = "x-audiolib-snapshot-format";
/// Full observed format descriptor paired with the exact format ID.
pub const LIBRARY_FORMAT_HEADER: &str = "x-audiolib-library-format";
/// Bound applies before parsing either peer's header JSON.
pub const MAX_COMPATIBILITY_HEADER_BYTES: usize = 4096;

/// Exact format descriptor accompanying `/library`, checked against the file later.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotFormat {
    /// Identifier recognized by both embedded catalogs.
    pub id: String,
    /// Full required feature map and observed storage structure.
    pub format: LibraryFormat,
}

impl SnapshotFormat {
    /// Accept only internally consistent descriptions explicitly readable by the catalog.
    pub fn is_readable_by(&self, manifest: &CompatibilityManifest) -> bool {
        self.format.snapshot_format_id().as_deref() == Some(self.id.as_str())
            && manifest.snapshot_read_formats.contains(&self.id)
    }
}

/// Search storage positively identified by inspecting the actual FTS definition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStorage {
    /// Legacy FTS reads its text from the business table.
    ExternalContent,
    /// FTS keeps postings/lengths; all input columns are supplied on replacement.
    ContentlessDelete,
    /// Absent, malformed, or an unfamiliar representation.
    Unknown,
}

/// Actual persistent format, including every declared feature (unknowns are retained).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LibraryFormat {
    /// SQLite user_version observed from the file.
    pub schema_version: u32,
    /// Observed FTS definition, not just a feature marker.
    pub storage: SearchStorage,
    /// All feature rows, including revisions this build does not know.
    pub features: BTreeMap<String, u32>,
    /// Source Metadata search extraction revision, if completed.
    pub source_search_revision: Option<u32>,
    /// Source tag extraction revision, if completed.
    pub source_tags_revision: Option<u32>,
}

impl LibraryFormat {
    /// Completed target format for plans and protocol negotiation.
    pub fn target() -> Self {
        Self {
            schema_version: TARGET_SCHEMA,
            storage: SearchStorage::ContentlessDelete,
            features: [
                ("stable_audio_ids", 1),
                ("foreign_key_indexes", 1),
                ("search_indexes", 1),
                ("tokenization", TOKENIZATION_REVISION),
                ("search_storage", 1),
                ("play_history_retired", 2),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
            source_search_revision: Some(2),
            source_tags_revision: Some(2),
        }
    }

    /// Exact completed snapshot format; incomplete/unknown combinations have no ID.
    pub fn snapshot_format_id(&self) -> Option<String> {
        if unknown_requirement(self).is_some()
            || self.features.get("tokenization") != Some(&TOKENIZATION_REVISION)
            || self.source_search_revision != Some(2)
            || self.source_tags_revision != Some(2)
        {
            return None;
        }
        match (&self.storage, self.schema_version) {
            (SearchStorage::ExternalContent, schema @ 3..=5)
                if !self.features.contains_key("search_storage")
                    && !self.features.contains_key("play_history_retired") =>
            {
                Some(format!("ale-s{schema}-ext-t3-m2"))
            }
            (SearchStorage::ContentlessDelete, TARGET_SCHEMA) => {
                if self == &Self::target() {
                    Some(PLAN_B_FORMAT.into())
                } else {
                    let mut initial = Self::target();
                    initial.features.insert("play_history_retired".into(), 1);
                    (self == &initial).then(|| PLAN_B_INITIAL_FORMAT.into())
                }
            }
            _ => None,
        }
    }
}

/// Durable maintenance marker observed by the database inspector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingMigration {
    /// Different protocols cannot resume each other's partial work.
    pub protocol: String,
    /// Protocol revision is independent of tokenization and schema.
    pub revision: u32,
    /// Completed target schema for the storage protocol, tokenizer for legacy rev 3.
    pub target: u32,
}

/// Required access determines whether an old readable format needs migration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryAccess {
    /// A validated legacy snapshot can remain unchanged.
    Read,
    /// This implementation writes only the target format.
    Write,
}

/// Three data dispositions; inability to decide is represented by None, not "rebuild".
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationDisposition {
    /// All operations requested by the caller are supported without conversion.
    Compatible,
    /// A supported conversion preserves business data except explicitly authorized retirement.
    Migratable,
    /// Dedicated inspection found unrecoverable business data, requiring recovery.
    RebuildRequired,
}

/// Execution constraints do not change the nature of a data migration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    /// No planning blocker is known; apply still rechecks locks, space and source.
    Ready,
    /// Resolve the reported blocker before proceeding.
    Blocked,
}

/// Machine-readable reason; adapters translate it without parsing an error string.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CompatibilityReason {
    /// Input is not an identified AudioLib database.
    InvalidLibrary,
    /// A newer/different tool or capability catalog is necessary.
    ToolUpgradeRequired,
    /// Required source business data is damaged or unavailable.
    BusinessDataUnrecoverable,
    /// A readable legacy source must first use the explicit owned import operation.
    LegacyImportRequired,
    /// Search state can be regenerated from retained business Metadata.
    DerivedStateUpgrade,
    /// The old write representation must be converted.
    StorageUpgrade,
    /// A known incomplete migration must finish or roll back first.
    ResumeRequired,
    /// Actual schema and feature markers disagree with structure.
    InvalidFormat,
}

/// Evidence supplied by the database layer after checking the pinned source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MigrationEvidence {
    /// True only after library identity and minimum business shape are checked.
    pub identified_library: bool,
    /// None means not checked; false requires a dedicated business-data failure.
    pub business_integrity: Option<bool>,
    /// True only when required Metadata is readable and sufficient for projection.
    pub can_regenerate_search: bool,
    /// Includes actual indexes, columns, FTS options and completed feature markers.
    pub search_ready: bool,
    /// Known or unknown durable state is never ignored.
    pub pending: Option<PendingMigration>,
}

/// A decision is not permission to mutate. Apply must bind and revalidate the source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompatibilityDecision {
    /// None for undiagnosed or unsupported inputs, never an invented rebuild verdict.
    pub disposition: Option<MigrationDisposition>,
    /// Space/permission/lease blockers can be added without changing disposition.
    pub execution_state: ExecutionState,
    /// Stable diagnostic reasons in evaluation order.
    pub reasons: Vec<CompatibilityReason>,
    /// True only for a completed, validated format supported for the requested access.
    pub direct_use: bool,
}

fn decision(
    disposition: Option<MigrationDisposition>,
    execution_state: ExecutionState,
    reasons: Vec<CompatibilityReason>,
) -> CompatibilityDecision {
    CompatibilityDecision {
        direct_use: disposition == Some(MigrationDisposition::Compatible)
            && execution_state == ExecutionState::Ready,
        disposition,
        execution_state,
        reasons,
    }
}

fn unknown_requirement(format: &LibraryFormat) -> Option<&str> {
    for (key, revision) in &format.features {
        let maximum = match key.as_str() {
            "stable_audio_ids" | "foreign_key_indexes" | "search_indexes" | "search_storage" => 1,
            "play_history_retired" => 2,
            "tokenization" => TOKENIZATION_REVISION,
            _ => return Some(key),
        };
        if *revision > maximum {
            return Some(key);
        }
    }
    if format.source_search_revision.is_some_and(|rev| rev > 2) {
        return Some("source_search_rev");
    }
    if format.source_tags_revision.is_some_and(|rev| rev > 2) {
        return Some("source_tags_rev");
    }
    None
}

/// Classify actual data independently of application version. This does no I/O.
pub fn assess_library(
    format: &LibraryFormat,
    evidence: &MigrationEvidence,
    access: LibraryAccess,
) -> CompatibilityDecision {
    use CompatibilityReason as Reason;
    use ExecutionState::{Blocked, Ready};
    use MigrationDisposition::{Compatible, Migratable, RebuildRequired};
    if !evidence.identified_library {
        return decision(None, Blocked, vec![Reason::InvalidLibrary]);
    }
    // Future formats cannot be diagnosed as corrupt merely because this tool
    // does not know their new columns, protocols or feature semantics.
    if format.schema_version > TARGET_SCHEMA {
        return decision(None, Blocked, vec![Reason::ToolUpgradeRequired]);
    }
    if let Some(pending) = &evidence.pending {
        let known = pending.revision == 1
            && ((pending.protocol == "search_text"
                && pending.target == 3
                && (1..=5).contains(&format.schema_version)
                && format.features.get("tokenization") == Some(&4))
                || (pending.protocol == "search_storage"
                    && pending.target == TARGET_SCHEMA
                    && (3..=TARGET_SCHEMA).contains(&format.schema_version)));
        // Tokenization=4 is permitted only within the existing rev-3 protocol.
        let mut completed_format = format.clone();
        if pending.protocol == "search_text" && format.features.get("tokenization") == Some(&4) {
            completed_format.features.insert("tokenization".into(), 3);
        }
        if !known || unknown_requirement(&completed_format).is_some() {
            return decision(None, Blocked, vec![Reason::ToolUpgradeRequired]);
        }
        if evidence.business_integrity == Some(false) {
            return decision(
                Some(RebuildRequired),
                Blocked,
                vec![Reason::BusinessDataUnrecoverable],
            );
        }
        return decision(Some(Migratable), Ready, vec![Reason::ResumeRequired]);
    }
    if unknown_requirement(format).is_some() {
        return decision(None, Blocked, vec![Reason::ToolUpgradeRequired]);
    }
    if evidence.business_integrity == Some(false) {
        return decision(
            Some(RebuildRequired),
            Blocked,
            vec![Reason::BusinessDataUnrecoverable],
        );
    }
    if (1..=2).contains(&format.schema_version) {
        return decision(
            Some(Migratable),
            Blocked,
            vec![Reason::LegacyImportRequired],
        );
    }
    if !(3..=TARGET_SCHEMA).contains(&format.schema_version) {
        return decision(None, Blocked, vec![Reason::InvalidFormat]);
    }
    if evidence.search_ready && format.snapshot_format_id().is_some() {
        if access == LibraryAccess::Read || format == &LibraryFormat::target() {
            return decision(Some(Compatible), Ready, vec![]);
        }
        return decision(Some(Migratable), Ready, vec![Reason::StorageUpgrade]);
    }
    if evidence.can_regenerate_search && evidence.business_integrity == Some(true) {
        return decision(Some(Migratable), Ready, vec![Reason::DerivedStateUpgrade]);
    }
    // Missing evidence must never be upgraded to an irreversible reconstruction.
    decision(None, Blocked, vec![Reason::InvalidFormat])
}

/// Embedded application/format catalog, exposed by both client and server.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct CompatibilityManifest {
    /// Shared workspace release identity; display, update checks and `blocked_versions`
    /// only. Never compared as a range.
    pub application_version: String,
    /// Present on marker-based peers (0.7.2+). Absent: the peer predates markers and
    /// must upgrade (Compatibility_Markers.md §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker_revision: Option<u32>,
    /// Supported API protocol interval; peers are compatible when intervals intersect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ProtocolRange>,
    /// Understood HTTP protocol identifiers (`api/v{n}`), kept beside `protocol` during
    /// the transition.
    #[serde(default)]
    pub api_protocols: Vec<String>,
    /// Complete snapshot format identifiers the application can read.
    #[serde(default)]
    pub snapshot_read_formats: Vec<String>,
    /// Complete database formats this application can write.
    #[serde(default)]
    pub database_write_formats: Vec<String>,
    /// Supported owned-conversion schema sources, not permission to mutate them.
    #[serde(default)]
    pub migration_source_schemas: Vec<u32>,
    /// Format produced by a successful Plan B conversion.
    #[serde(default)]
    pub migration_target_format: String,
    /// Known persistent migration protocols.
    #[serde(default)]
    pub migration_protocols: BTreeMap<String, u32>,
    /// Optional capabilities `{name: revision}`. A function is used only when both
    /// sides declare it at the same revision; a missing one never refuses sync.
    #[serde(default)]
    pub features: BTreeMap<String, u32>,
    /// Known-bad application versions refused regardless of markers.
    #[serde(default)]
    pub blocked_versions: Vec<String>,
    /// Informational; never compared for equality.
    #[serde(default)]
    pub catalog_revision: u32,
    /// Declared for peers still doing range checks (they ignore markers); never used
    /// for decisions here.
    #[serde(default)]
    pub peer_application_ranges: Vec<String>,
    /// Display/diagnostics only; each side checks its own linked SQLite at startup
    /// ([`check_linked_sqlite`]).
    #[serde(default)]
    pub minimum_sqlite_version: String,
}

/// Inclusive API protocol interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ProtocolRange {
    pub min: u32,
    pub max: u32,
}

/// Current marker scheme revision.
pub const MARKER_REVISION: u32 = 1;
/// API protocol interval of this build (`api/v1`).
pub const PROTOCOL: ProtocolRange = ProtocolRange { min: 1, max: 1 };
/// Known-bad application versions. Intentionally empty; add an exact version only for a
/// published release that corrupts peers.
pub const BLOCKED_VERSIONS: &[&str] = &[];
/// Optional features and revisions this build implements on both client and server.
pub const FEATURES: &[(&str, u32)] = &[
    ("list_waveform_jobs", 1),
    ("list_waveforms", 1),
    ("waveform_bundle_strong_etag", 1),
];
/// Accepted-range shape kept for peers that still do range checks.
pub const PEER_APPLICATION_RANGES: &[&str] = &[">=0.6.0"];

/// Snapshot capability-max request headers and their values (derived, never hand-written),
/// shared by the client sender and the server's admission check.
pub const SNAPSHOT_SCHEMA_MAX_HEADER: &str = "x-audiolib-snapshot-schema-max";
pub const SEARCH_STORAGE_MAX_HEADER: &str = "x-audiolib-search-storage-max";
pub const TOKENIZATION_MAX_HEADER: &str = "x-audiolib-tokenization-max";
/// Highest schema the client reads.
pub const SNAPSHOT_SCHEMA_MAX: u32 = TARGET_SCHEMA;
/// Highest search storage revision the client reads (matches `LibraryFormat::target()`).
pub const SEARCH_STORAGE_MAX: u32 = 1;
/// Highest tokenization revision the client reads.
pub const TOKENIZATION_MAX: u32 = TOKENIZATION_REVISION;

/// Error codes (409) for refused peers.
pub const PEER_UPGRADE_REQUIRED: &str = "PEER_UPGRADE_REQUIRED";
pub const PROTOCOL_UNSUPPORTED: &str = "PROTOCOL_UNSUPPORTED";
pub const PEER_VERSION_BLOCKED: &str = "PEER_VERSION_BLOCKED";
/// Pre-marker code still sent by 0.6.x–0.7.1 servers; also used for an inconsistent catalog.
pub const PEER_VERSION_UNSUPPORTED: &str = "PEER_VERSION_UNSUPPORTED";

/// Why a marker check refused a peer (Compatibility_Markers.md §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerRejection {
    /// The peer's version is on our `blocked_versions` list.
    BlockedVersion,
    /// No catalog or no `marker_revision`: the peer predates markers.
    PeerUpgradeRequired,
    /// The catalog's `application_version` differs from the reported version.
    Inconsistent,
    /// Peer protocol entirely below ours: the peer must upgrade.
    ProtocolPeerTooOld,
    /// Peer protocol entirely above ours (peer min > our max): this side must upgrade.
    ProtocolPeerTooNew,
}

impl PeerRejection {
    /// HTTP error code a server sends for this refusal of a client.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::BlockedVersion => PEER_VERSION_BLOCKED,
            Self::PeerUpgradeRequired | Self::ProtocolPeerTooOld => PEER_UPGRADE_REQUIRED,
            Self::ProtocolPeerTooNew => PROTOCOL_UNSUPPORTED,
            Self::Inconsistent => PEER_VERSION_UNSUPPORTED,
        }
    }
}

/// Fail fast when the linked SQLite is older than [`MINIMUM_SQLITE_VERSION`]. Called by
/// each process at startup for its own engine; never used to judge a peer.
pub fn check_linked_sqlite(linked: &str) -> Result<(), String> {
    let parse = |text: &str| -> Option<Version> {
        let mut parts = text.trim().split('.').map(|part| part.parse::<u64>().ok());
        Some(Version::new(
            parts.next()??,
            parts.next().flatten().unwrap_or(0),
            parts.next().flatten().unwrap_or(0),
        ))
    };
    match (parse(linked), parse(MINIMUM_SQLITE_VERSION)) {
        (Some(found), Some(minimum)) if found >= minimum => Ok(()),
        _ => Err(format!(
            "Linked SQLite {linked} is older than the required {MINIMUM_SQLITE_VERSION}; rebuild with the bundled SQLite"
        )),
    }
}

/// Marker-based peer check: no version ranges, no catalog_revision equality.
pub fn check_peer(
    local: &CompatibilityManifest,
    peer_version: &str,
    peer: Option<&CompatibilityManifest>,
) -> Result<(), PeerRejection> {
    if local.blocked_versions.iter().any(|v| v == peer_version) {
        return Err(PeerRejection::BlockedVersion);
    }
    let Some(peer) = peer.filter(|peer| peer.marker_revision.is_some()) else {
        return Err(PeerRejection::PeerUpgradeRequired);
    };
    if peer.application_version != peer_version {
        return Err(PeerRejection::Inconsistent);
    }
    let local_range = local.protocol.unwrap_or(PROTOCOL);
    let Some(peer_range) = peer.protocol else {
        return Err(PeerRejection::PeerUpgradeRequired);
    };
    if peer_range.max < local_range.min {
        return Err(PeerRejection::ProtocolPeerTooOld);
    }
    if peer_range.min > local_range.max {
        return Err(PeerRejection::ProtocolPeerTooNew);
    }
    // Transitional: `api_protocols` must agree with the interval.
    if !peer
        .api_protocols
        .iter()
        .any(|protocol| local.api_protocols.contains(protocol))
    {
        return Err(if peer_range.max > local_range.max {
            PeerRejection::ProtocolPeerTooNew
        } else {
            PeerRejection::ProtocolPeerTooOld
        });
    }
    Ok(())
}

/// This build's catalog (Compatibility_Markers.md §3).
pub fn compatibility_manifest() -> CompatibilityManifest {
    CompatibilityManifest {
        application_version: env!("CARGO_PKG_VERSION").into(),
        marker_revision: Some(MARKER_REVISION),
        protocol: Some(PROTOCOL),
        api_protocols: vec![API_PROTOCOL.into()],
        snapshot_read_formats: vec![
            "ale-s3-ext-t3-m2".into(),
            "ale-s4-ext-t3-m2".into(),
            "ale-s5-ext-t3-m2".into(),
            PLAN_B_INITIAL_FORMAT.into(),
            PLAN_B_FORMAT.into(),
        ],
        database_write_formats: vec![PLAN_B_FORMAT.into()],
        migration_source_schemas: vec![3, 4, 5, TARGET_SCHEMA],
        migration_target_format: PLAN_B_FORMAT.into(),
        migration_protocols: [("search_text".into(), 1), ("search_storage".into(), 1)]
            .into_iter()
            .collect(),
        features: FEATURES
            .iter()
            .map(|(name, revision)| ((*name).to_owned(), *revision))
            .collect(),
        blocked_versions: BLOCKED_VERSIONS.iter().map(|v| (*v).to_owned()).collect(),
        catalog_revision: CATALOG_REVISION,
        peer_application_ranges: PEER_APPLICATION_RANGES
            .iter()
            .map(|range| (*range).to_owned())
            .collect(),
        minimum_sqlite_version: MINIMUM_SQLITE_VERSION.into(),
    }
}

/// Whether `peer` may sync with `local` ([`check_peer`] without the reason).
pub fn peer_versions_compatible(
    local: &CompatibilityManifest,
    peer_version: &str,
    peer: Option<&CompatibilityManifest>,
) -> bool {
    check_peer(local, peer_version, peer).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> MigrationEvidence {
        MigrationEvidence {
            identified_library: true,
            business_integrity: Some(true),
            can_regenerate_search: true,
            search_ready: true,
            pending: None,
        }
    }

    #[test]
    fn read_compatibility_does_not_grant_legacy_write_compatibility() {
        let mut format = LibraryFormat::target();
        format.schema_version = 5;
        format.storage = SearchStorage::ExternalContent;
        format.features.remove("search_storage");
        format.features.remove("play_history_retired");
        assert!(assess_library(&format, &evidence(), LibraryAccess::Read).direct_use);
        assert_eq!(
            assess_library(&format, &evidence(), LibraryAccess::Write).reasons,
            vec![CompatibilityReason::StorageUpgrade]
        );
        assert!(
            assess_library(&LibraryFormat::target(), &evidence(), LibraryAccess::Write).direct_use
        );
    }

    #[test]
    fn prior_schema6_requires_timestamp_retirement_for_writes() {
        let mut initial = LibraryFormat::target();
        initial.features.insert("play_history_retired".into(), 1);
        assert_eq!(
            initial.snapshot_format_id().as_deref(),
            Some(PLAN_B_INITIAL_FORMAT)
        );
        assert!(
            compatibility_manifest()
                .snapshot_read_formats
                .contains(&PLAN_B_INITIAL_FORMAT.into())
        );
        assert!(assess_library(&initial, &evidence(), LibraryAccess::Read).direct_use);
        assert_eq!(
            assess_library(&initial, &evidence(), LibraryAccess::Write).disposition,
            Some(MigrationDisposition::Migratable)
        );
        initial.features.insert("play_history_retired".into(), 3);
        assert_eq!(initial.snapshot_format_id(), None);
        assert_eq!(
            assess_library(&initial, &evidence(), LibraryAccess::Write).reasons,
            vec![CompatibilityReason::ToolUpgradeRequired]
        );
    }

    #[test]
    fn unknown_future_and_incomplete_evidence_never_imply_rebuild() {
        for (key, value) in [("tokenization", 4), ("unknown_future_feature", 1)] {
            let mut format = LibraryFormat::target();
            format.features.insert(key.into(), value);
            let result = assess_library(&format, &evidence(), LibraryAccess::Write);
            assert_eq!(result.disposition, None);
            assert_eq!(
                result.reasons,
                vec![CompatibilityReason::ToolUpgradeRequired]
            );
        }
        let mut proof = evidence();
        proof.search_ready = false;
        proof.business_integrity = None;
        assert_eq!(
            assess_library(&LibraryFormat::target(), &proof, LibraryAccess::Write).disposition,
            None
        );
        proof.business_integrity = Some(false);
        assert_eq!(
            assess_library(&LibraryFormat::target(), &proof, LibraryAccess::Write).disposition,
            Some(MigrationDisposition::RebuildRequired)
        );
    }

    #[test]
    fn legacy_in_progress_tokenization_four_is_not_a_completed_new_codec() {
        let mut format = LibraryFormat::target();
        format.schema_version = 5;
        format.storage = SearchStorage::ExternalContent;
        format.features.remove("search_storage");
        format.features.remove("play_history_retired");
        format.features.insert("tokenization".into(), 4);
        let mut proof = evidence();
        proof.pending = Some(PendingMigration {
            protocol: "search_text".into(),
            revision: 1,
            target: 3,
        });
        let result = assess_library(&format, &proof, LibraryAccess::Write);
        assert_eq!(result.disposition, Some(MigrationDisposition::Migratable));
        assert_eq!(result.reasons, vec![CompatibilityReason::ResumeRequired]);
        proof.pending.as_mut().unwrap().revision = 99;
        assert_eq!(
            assess_library(&format, &proof, LibraryAccess::Write).disposition,
            None
        );
    }

    fn marked(version: &str) -> CompatibilityManifest {
        let mut catalog = compatibility_manifest();
        catalog.application_version = version.into();
        catalog
    }

    /// Real shipped catalogs that predate markers (fixtures, not derived from this build).
    fn shipped(version: &str) -> CompatibilityManifest {
        let json = std::fs::read_to_string(format!(
            "{}/tests/fixtures/compatibility/{version}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn matrix_1_and_4_same_protocol_marker_peers_pass_regardless_of_version() {
        let local = compatibility_manifest();
        for version in [
            local.application_version.as_str(),
            "0.7.2",
            "0.8.0",
            "9.0.0",
        ] {
            assert_eq!(check_peer(&local, version, Some(&marked(version))), Ok(()));
            assert!(peer_versions_compatible(
                &marked(version),
                &local.application_version,
                Some(&local)
            ));
        }
    }

    #[test]
    fn matrix_2_3_and_10_unmarked_peers_require_upgrade() {
        let local = compatibility_manifest();
        for version in ["0.6.5", "0.7.0", "0.7.1"] {
            let old = shipped(version);
            assert_eq!(old.marker_revision, None, "{version}");
            assert_eq!(
                check_peer(&local, version, Some(&old)),
                Err(PeerRejection::PeerUpgradeRequired),
                "{version}"
            );
            assert_eq!(
                PeerRejection::PeerUpgradeRequired.error_code(),
                PEER_UPGRADE_REQUIRED
            );
        }
        // 0.4.11/0.5.x have no catalog at all; the legacy path is gone.
        for version in ["0.4.11", "0.5.0", "0.5.1"] {
            assert_eq!(
                check_peer(&local, version, None),
                Err(PeerRejection::PeerUpgradeRequired)
            );
        }
    }

    #[test]
    fn matrix_5_disjoint_protocols_name_the_side_that_must_upgrade() {
        let local = compatibility_manifest();
        let mut newer = marked("0.9.0");
        newer.protocol = Some(ProtocolRange { min: 2, max: 3 });
        newer.api_protocols = vec!["api/v2".into(), "api/v3".into()];
        assert_eq!(
            check_peer(&local, "0.9.0", Some(&newer)),
            Err(PeerRejection::ProtocolPeerTooNew)
        );
        assert_eq!(
            PeerRejection::ProtocolPeerTooNew.error_code(),
            PROTOCOL_UNSUPPORTED
        );
        // Seen from the newer side, we are the one too old.
        assert_eq!(
            check_peer(&newer, &local.application_version, Some(&local)),
            Err(PeerRejection::ProtocolPeerTooOld)
        );
        assert_eq!(
            PeerRejection::ProtocolPeerTooOld.error_code(),
            PEER_UPGRADE_REQUIRED
        );
        // Overlapping intervals pass.
        let mut wide = marked("0.9.0");
        wide.protocol = Some(ProtocolRange { min: 1, max: 2 });
        wide.api_protocols = vec!["api/v1".into(), "api/v2".into()];
        assert_eq!(check_peer(&local, "0.9.0", Some(&wide)), Ok(()));
        // Interval and identifiers must agree during the transition.
        let mut stale = marked("0.9.0");
        stale.api_protocols = vec!["api/v99".into()];
        assert!(check_peer(&local, "0.9.0", Some(&stale)).is_err());
    }

    #[test]
    fn matrix_6_catalog_revision_and_ranges_are_informational() {
        let local = compatibility_manifest();
        let mut peer = marked("0.7.9");
        peer.catalog_revision += 7;
        peer.peer_application_ranges = vec!["<0.1.0".into()];
        assert_eq!(check_peer(&local, "0.7.9", Some(&peer)), Ok(()));
    }

    #[test]
    fn matrix_9_blocked_versions_refuse_first() {
        let mut local = compatibility_manifest();
        assert!(local.blocked_versions.is_empty() && BLOCKED_VERSIONS.is_empty());
        local.blocked_versions = vec!["0.7.3".into()];
        assert_eq!(
            check_peer(&local, "0.7.3", Some(&marked("0.7.3"))),
            Err(PeerRejection::BlockedVersion)
        );
        assert_eq!(
            PeerRejection::BlockedVersion.error_code(),
            PEER_VERSION_BLOCKED
        );
    }

    #[test]
    fn inconsistent_catalog_is_refused() {
        let local = compatibility_manifest();
        assert_eq!(
            check_peer(&local, "0.9.9", Some(&marked("0.8.0"))),
            Err(PeerRejection::Inconsistent)
        );
    }

    #[test]
    fn matrix_12_feature_revisions_are_per_feature() {
        let local = compatibility_manifest();
        let mut peer = marked("0.8.0");
        peer.features.insert("list_waveforms".into(), 2);
        peer.features.remove("waveform_bundle_strong_etag");
        peer.features.insert("future_optional".into(), 1);
        // Features never refuse the peer.
        assert_eq!(check_peer(&local, "0.8.0", Some(&peer)), Ok(()));
    }

    #[test]
    fn unknown_fields_are_ignored_and_old_catalogs_still_parse() {
        let mut json = serde_json::to_value(compatibility_manifest()).unwrap();
        json["future_marker"] = serde_json::json!({"x": 1});
        let parsed: CompatibilityManifest = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, compatibility_manifest());
        for version in ["0.6.5", "0.7.0", "0.7.1"] {
            assert!(shipped(version).features.is_empty());
        }
    }

    #[test]
    fn manifest_fits_the_header_bound() {
        let json = serde_json::to_string(&compatibility_manifest()).unwrap();
        assert!(
            json.len() <= MAX_COMPATIBILITY_HEADER_BYTES,
            "{}",
            json.len()
        );
    }

    #[test]
    fn linked_sqlite_self_check() {
        assert!(check_linked_sqlite(MINIMUM_SQLITE_VERSION).is_ok());
        assert!(check_linked_sqlite("3.60.0").is_ok());
        assert!(check_linked_sqlite("3.54.0").is_err());
        assert!(check_linked_sqlite("garbage").is_err());
    }

    /// Format fingerprint fixture: changing any format input must also change its ID
    /// (Compatibility_Markers.md §5.2). Update the fixture together with a new format ID.
    #[test]
    fn format_fingerprints_match_fixture() {
        let target = LibraryFormat::target();
        let actual = serde_json::json!({
            "target_id": target.snapshot_format_id(),
            "target": target,
            "plan_b_format": PLAN_B_FORMAT,
            "plan_b_initial_format": PLAN_B_INITIAL_FORMAT,
            "target_schema": TARGET_SCHEMA,
            "tokenization_revision": TOKENIZATION_REVISION,
            "snapshot_read_formats": compatibility_manifest().snapshot_read_formats,
            "capability_max": {
                SNAPSHOT_SCHEMA_MAX_HEADER: SNAPSHOT_SCHEMA_MAX,
                SEARCH_STORAGE_MAX_HEADER: SEARCH_STORAGE_MAX,
                TOKENIZATION_MAX_HEADER: TOKENIZATION_MAX,
            },
        });
        assert_eq!(actual["target_id"], PLAN_B_FORMAT);
        assert_eq!(
            target.features.get("search_storage").copied(),
            Some(SEARCH_STORAGE_MAX)
        );
        snapshot("format_fingerprint.json", &actual);
    }

    /// DTO JSON-schema snapshot: any manifest shape change shows up in review and must be
    /// classified per §5.2 (new optional field vs protocol bump).
    #[test]
    fn manifest_json_schema_snapshot() {
        let schema = schemars::schema_for!(CompatibilityManifest);
        snapshot(
            "compatibility_manifest.schema.json",
            &serde_json::to_value(schema).unwrap(),
        );
    }

    /// Compare with `tests/fixtures/compatibility/<name>`; `AUDIOLIB_UPDATE_SNAPSHOTS=1`
    /// rewrites it.
    fn snapshot(name: &str, actual: &serde_json::Value) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/compatibility")
            .join(name);
        let text = serde_json::to_string_pretty(actual).unwrap() + "\n";
        if std::env::var_os("AUDIOLIB_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert_eq!(
            expected, text,
            "{name} changed; review and set AUDIOLIB_UPDATE_SNAPSHOTS=1"
        );
    }

    #[test]
    fn unrelated_file_and_legacy_import_are_never_automatic_replacements() {
        let mut proof = evidence();
        proof.identified_library = false;
        assert_eq!(
            assess_library(&LibraryFormat::target(), &proof, LibraryAccess::Write).reasons,
            vec![CompatibilityReason::InvalidLibrary]
        );
        proof.identified_library = true;
        let mut format = LibraryFormat::target();
        format.schema_version = 2;
        assert_eq!(
            assess_library(&format, &proof, LibraryAccess::Write).execution_state,
            ExecutionState::Blocked
        );
    }
}
