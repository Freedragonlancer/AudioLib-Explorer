// crates/audiolib-core/src/compatibility.rs
//! Owned, conservative compatibility decisions shared by every application adapter.
//! Database observations must come from a pinned inspection, never from SemVer alone.
use semver::{Version, VersionReq};
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
pub struct CompatibilityManifest {
    /// Shared workspace release identity.
    pub application_version: String,
    /// Capability semantics, separate from database schema.
    pub catalog_revision: u32,
    /// Each entry is an explicit semver requirement; pre-releases require opt-in.
    pub peer_application_ranges: Vec<String>,
    /// Tested published peers that predate the catalog. This exception is bounded
    /// independently of modern peers; an absent field grants no legacy admission.
    #[serde(default)]
    pub legacy_peer_application_ranges: Vec<String>,
    /// Tested application origins that may directly use supported read formats.
    pub direct_use_application_ranges: Vec<String>,
    /// Tested application origins with supported owned migration routes.
    pub auto_migrate_application_ranges: Vec<String>,
    /// Understood HTTP protocol identifiers.
    pub api_protocols: Vec<String>,
    /// Complete snapshot format identifiers the application can read.
    pub snapshot_read_formats: Vec<String>,
    /// Complete database formats this application can write.
    pub database_write_formats: Vec<String>,
    /// Supported owned-conversion schema sources, not permission to mutate them.
    pub migration_source_schemas: Vec<u32>,
    /// Format produced by a successful Plan B conversion.
    pub migration_target_format: String,
    /// Known persistent migration protocols.
    pub migration_protocols: BTreeMap<String, u32>,
    /// Required statistics-fix baseline.
    pub minimum_sqlite_version: String,
}

/// Exact published protocol baselines. Keep legacy admission and the exposed
/// direct-use/migration ranges derived from this one reviewed release set.
const LEGACY_APPLICATION_RANGES: &[&str] = &["=0.4.11", "=0.5.0", "=0.5.1"];

/// This development catalog admits the tested published releases and the exact
/// development version. Actual format inspection still decides direct use versus
/// owned migration; neither the version nor these overlapping ranges grants writes.
pub fn compatibility_manifest() -> CompatibilityManifest {
    let legacy_ranges: Vec<String> = LEGACY_APPLICATION_RANGES
        .iter()
        .map(|requirement| (*requirement).to_owned())
        .collect();
    let mut supported_ranges = legacy_ranges.clone();
    supported_ranges.push(format!("={}", env!("CARGO_PKG_VERSION")));
    CompatibilityManifest {
        application_version: env!("CARGO_PKG_VERSION").into(),
        catalog_revision: CATALOG_REVISION,
        peer_application_ranges: supported_ranges.clone(),
        legacy_peer_application_ranges: legacy_ranges,
        direct_use_application_ranges: supported_ranges.clone(),
        auto_migrate_application_ranges: supported_ranges,
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
        minimum_sqlite_version: MINIMUM_SQLITE_VERSION.into(),
    }
}

fn accepts_range(ranges: &[String], version: &Version) -> bool {
    ranges.iter().any(|requirement| {
        VersionReq::parse(requirement).is_ok_and(|requirement| requirement.matches(version))
    })
}

/// Validate both directions of a modern handshake. Only releases explicitly
/// recorded in the local legacy catalog may omit theirs; all downloaded formats
/// still require independent header/file checks.
pub fn peer_versions_compatible(
    local: &CompatibilityManifest,
    peer_version: &str,
    peer: Option<&CompatibilityManifest>,
) -> bool {
    let (Ok(peer_version), Ok(local_version)) = (
        Version::parse(peer_version),
        Version::parse(&local.application_version),
    ) else {
        return false;
    };
    if !accepts_range(&local.peer_application_ranges, &peer_version) {
        return false;
    }
    match peer {
        Some(peer) => {
            Version::parse(&peer.application_version).ok().as_ref() == Some(&peer_version)
                && peer.catalog_revision == CATALOG_REVISION
                && accepts_range(&peer.peer_application_ranges, &local_version)
                && peer
                    .api_protocols
                    .iter()
                    .any(|p| local.api_protocols.contains(p))
        }
        None => accepts_range(&local.legacy_peer_application_ranges, &peer_version),
    }
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

    #[test]
    fn peer_checks_are_mutual_bounded_and_pre_release_explicit() {
        let local = compatibility_manifest();
        for supported in ["0.4.11", "0.5.0", "0.5.1"] {
            assert!(peer_versions_compatible(&local, supported, None));
            let version = Version::parse(supported).unwrap();
            for ranges in [
                &local.peer_application_ranges,
                &local.direct_use_application_ranges,
                &local.auto_migrate_application_ranges,
            ] {
                assert!(accepts_range(ranges, &version));
            }
        }
        for unsupported in [
            "0.4.10",
            "0.4.12",
            "0.4.11-rc.1",
            "0.5.0-rc.1",
            "0.5.1-rc.1",
            "0.5.2",
            "0.6.1",
            "0.6.0-dev.2",
            "9.0.0",
            "invalid",
        ] {
            assert!(!peer_versions_compatible(&local, unsupported, None));
        }
        // A current prerelease opts into modern mutual negotiation only. Even an
        // otherwise identical process cannot silently omit its capability catalog.
        assert!(!peer_versions_compatible(
            &local,
            &local.application_version,
            None
        ));
        assert!(peer_versions_compatible(
            &local,
            &local.application_version,
            Some(&local)
        ));
        let mut peer = local.clone();
        peer.peer_application_ranges = vec!["<0.4.0".into()];
        assert!(!peer_versions_compatible(
            &local,
            &peer.application_version,
            Some(&peer)
        ));
    }

    #[test]
    fn catalog_controls_legacy_exception_without_bypassing_peer_ranges() {
        let mut local = compatibility_manifest();
        local.legacy_peer_application_ranges.clear();
        for supported in ["0.4.11", "0.5.0", "0.5.1"] {
            assert!(!peer_versions_compatible(&local, supported, None));
        }
        // A catalog does not classify an untested release as legacy merely because
        // its ordinary peer range admits that version.
        local.peer_application_ranges.push("=0.5.2".into());
        assert!(!peer_versions_compatible(&local, "0.5.2", None));
        local.legacy_peer_application_ranges = vec!["=0.5.0".into()];
        assert!(peer_versions_compatible(&local, "0.5.0", None));
        local.peer_application_ranges = vec![format!("={}", local.application_version)];
        assert!(!peer_versions_compatible(&local, "0.5.0", None));
    }

    #[test]
    fn absent_legacy_catalog_is_conservative_without_breaking_modern_peers() {
        let local = compatibility_manifest();
        let mut json = serde_json::to_value(&local).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("legacy_peer_application_ranges");
        let older_catalog: CompatibilityManifest = serde_json::from_value(json).unwrap();
        assert!(older_catalog.legacy_peer_application_ranges.is_empty());
        for supported in ["0.4.11", "0.5.0", "0.5.1"] {
            assert!(!peer_versions_compatible(&older_catalog, supported, None));
        }
        assert!(peer_versions_compatible(
            &local,
            &older_catalog.application_version,
            Some(&older_catalog)
        ));
    }

    #[test]
    fn modern_peer_must_match_identity_catalog_and_protocol_in_both_directions() {
        let local = compatibility_manifest();
        for mode in ["identity", "catalog", "protocol", "range", "invalid_range"] {
            let mut peer = local.clone();
            match mode {
                "identity" => peer.application_version = "0.5.0".into(),
                "catalog" => peer.catalog_revision += 1,
                "protocol" => peer.api_protocols = vec!["api/v99".into()],
                "range" => peer.peer_application_ranges = vec!["=0.5.0".into()],
                "invalid_range" => peer.peer_application_ranges = vec!["invalid".into()],
                _ => unreachable!(),
            }
            assert!(
                !peer_versions_compatible(&local, &local.application_version, Some(&peer)),
                "{mode}"
            );
        }
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
