//! Client view of a server that is newer, older or migrating its library schema
//! (DESIGN-SEARCH-METADATA-V2 §5.3, §6, §7). Pure decisions only: no I/O, the remote
//! crate gathers the inputs (`/capabilities`, `/health`, response headers).
//!
//! The client never downloads or installs anything here; it only reports which
//! notices to show and whether path search may be offered.
use crate::compatibility;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Response header a newer server adds for an older client (§6, non-blocking).
pub const UPGRADE_AVAILABLE_HEADER: &str = "x-audiolib-upgrade-available";
/// Longest header value accepted; longer values are ignored.
pub const MAX_UPGRADE_HEADER_BYTES: usize = 64;
/// The schema 9 Plan B snapshot format this client recognizes (§3, §6).
pub const PLAN_B_V9_FORMAT: &str = "ale-s9-cld1-t3-m2-p1-a1-sp1";
/// Path search rules this client knows (`sp1`, `search_path: 1`).
pub const SEARCH_PATH_REVISION: u32 = 1;
/// Compatibility feature names introduced by the design (§6).
pub const SEARCH_PATH_FEATURE: &str = "search_path";
pub const VORBIS_TAGS_FEATURE: &str = "vorbis_tags";
pub const VENDOR_METADATA_FEATURE: &str = "vendor_metadata";
/// Highest schema and the tokenization this client understands.
pub const MAX_KNOWN_SCHEMA: u32 = 9;

/// A parsed `x-audiolib-upgrade-available` header.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeAvailable {
    /// Version the server names, when the value looks like one.
    pub version: Option<String>,
}

/// Parse the header value. Empty, oversized or non-printable values are ignored; a
/// value that is not a version still means "an upgrade exists" without naming one.
pub fn parse_upgrade_available(value: &str) -> Option<UpgradeAvailable> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_UPGRADE_HEADER_BYTES
        || !value.chars().all(|c| c.is_ascii_graphic())
    {
        return None;
    }
    let version_like = value.trim_start_matches(['v', 'V']);
    let is_version = version_like.starts_with(|c: char| c.is_ascii_digit())
        && version_like
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    Some(UpgradeAvailable {
        version: is_version.then(|| version_like.to_string()),
    })
}

/// Segments of a snapshot format ID such as `ale-s9-cld1-t3-m2-p1-a1-sp1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatId {
    pub schema: u32,
    pub tokenization: u32,
    /// `sp{n}`; absent before schema 9.
    pub search_path: Option<u32>,
}

/// Parse the segments this client reasons about; `None` for anything malformed.
pub fn parse_format_id(id: &str) -> Option<FormatId> {
    let mut parts = id.split('-');
    if parts.next()? != "ale" {
        return None;
    }
    let number = |part: &str, prefix: &str| -> Option<u32> {
        let digits = part.strip_prefix(prefix)?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| digits.parse().ok())
            .flatten()
    };
    let schema = number(parts.next()?, "s")?;
    let mut tokenization = None;
    let mut search_path = None;
    for part in parts {
        if part.is_empty() {
            return None;
        }
        if let Some(value) = number(part, "sp") {
            if search_path.replace(value).is_some() {
                return None;
            }
        } else if let Some(value) = number(part, "t") {
            if tokenization.replace(value).is_some() {
                return None;
            }
        } else if !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
    }
    Some(FormatId {
        schema,
        tokenization: tokenization?,
        search_path,
    })
}

/// How this client relates to one snapshot format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatSupport {
    /// A format this build reads today.
    Readable,
    /// The schema 9 `sp1` format: recognized, path search rules known.
    KnownPathSearch,
    /// Main tables known but newer path search rules (`sp2`+): never query them with
    /// `sp1` rules.
    NewerPathRules,
    /// Unknown schema/tokenization major or malformed: incompatible.
    Incompatible,
}

pub fn format_support(id: &str) -> FormatSupport {
    let manifest = compatibility::compatibility_manifest();
    if manifest
        .snapshot_read_formats
        .iter()
        .any(|known| known == id)
    {
        return FormatSupport::Readable;
    }
    if id == PLAN_B_V9_FORMAT {
        return FormatSupport::KnownPathSearch;
    }
    match parse_format_id(id) {
        Some(format)
            if format.schema == MAX_KNOWN_SCHEMA
                && format.tokenization == compatibility::TOKENIZATION_REVISION
                && format
                    .search_path
                    .is_some_and(|sp| sp > SEARCH_PATH_REVISION) =>
        {
            FormatSupport::NewerPathRules
        }
        _ => FormatSupport::Incompatible,
    }
}

/// `/capabilities.library_schema` (§6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibrarySchemaInfo {
    #[serde(default)]
    pub current: Option<u32>,
    #[serde(default)]
    pub source_search_rev: Option<u32>,
    #[serde(default)]
    pub snapshot_formats_served: Vec<String>,
}

/// Migration state values (§5.3); unknown values are kept as `Unknown`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationState {
    #[default]
    Idle,
    Running,
    Paused,
    Failed,
    Done,
    #[serde(other)]
    Unknown,
}

impl MigrationState {
    pub fn in_progress(self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }
}

/// `/capabilities.schema_migration` (§6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaMigrationInfo {
    #[serde(default)]
    pub state: MigrationState,
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// `/capabilities.vendor_import` (§6); limits are the server's, never assumed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorImportInfo {
    pub revision: u32,
    pub max_file_bytes: u64,
    pub max_rows: u64,
}

/// `/health.upgrade`: `{phase, completed?, total?}` mirrored for every client.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthUpgrade {
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub completed: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
}

/// `GET /admin/api/migration` (§5.3). `rollback_backup` is a server path: never shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationStatus {
    #[serde(default)]
    pub state: MigrationState,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub completed: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub batch_ms_p50: Option<u64>,
    #[serde(default)]
    pub eta_s: Option<u64>,
    #[serde(default, skip_serializing)]
    pub rollback_backup: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Everything the evaluation looks at, gathered by the caller.
#[derive(Clone, Debug, Default)]
pub struct ServerUpgradeInput {
    pub server_version: Option<String>,
    /// `/capabilities` was missing or unreadable: treat as an older server (VN-04).
    pub capabilities_known: bool,
    pub features: BTreeMap<String, u32>,
    pub library_schema: Option<LibrarySchemaInfo>,
    pub schema_migration: Option<SchemaMigrationInfo>,
    pub vendor_import: Option<VendorImportInfo>,
    pub health_upgrade: Option<HealthUpgrade>,
    pub admin_migration: Option<MigrationStatus>,
    /// Format IDs actually observed for the snapshot in use (library descriptor and
    /// snapshot response header); disagreement disables path search (VN-12).
    pub observed_formats: Vec<String>,
    pub upgrade_available: Option<UpgradeAvailable>,
    pub can_manage: bool,
}

/// Whether path search may be offered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathSearch {
    Available,
    /// Older or migrating server: hidden silently (§6, §7.2 "only when clicked").
    NotOffered,
    /// The server uses newer path rules than this client: hidden, prompt upgrade.
    NeedsClientUpgrade,
}

/// A migration the status bar shows (§7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationProgress {
    pub state: MigrationState,
    pub phase: Option<String>,
    /// `completed / total`, when both are known.
    pub percent: Option<u8>,
}

/// Notices, each shown at most once per session (keyed by [`Notice::key`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notice {
    /// §7.6: the server advertised a newer client.
    ClientUpgradeAvailable { version: Option<String> },
    /// VN-12: newer path rules; main search keeps working.
    PathSearchNeedsUpgrade,
    /// §7.3: migration in progress.
    Migrating { percent: Option<u8> },
    /// §7.2: nothing the server offers is readable.
    Incompatible { server_version: Option<String> },
}

impl Notice {
    /// Session de-duplication key (progress changes do not re-show a notice).
    pub fn key(&self) -> &'static str {
        match self {
            Self::ClientUpgradeAvailable { .. } => "client-upgrade",
            Self::PathSearchNeedsUpgrade => "path-search-upgrade",
            Self::Migrating { .. } => "migrating",
            Self::Incompatible { .. } => "incompatible",
        }
    }

    /// Offer the existing "check for updates" action (never auto-install).
    pub fn offers_update_check(&self) -> bool {
        !matches!(self, Self::Migrating { .. })
    }

    pub fn text(&self, chinese: bool) -> String {
        match (self, chinese) {
            (Self::ClientUpgradeAvailable { .. }, true) => {
                "服务器已升级，更新客户端可使用路径搜索和厂商资料".into()
            }
            (Self::ClientUpgradeAvailable { version }, false) => match version {
                Some(version) => format!(
                    "The server was upgraded; update the client ({version}) to use path search and vendor metadata"
                ),
                None => "The server was upgraded; update the client to use path search and vendor metadata".into(),
            },
            (Self::PathSearchNeedsUpgrade, true) => {
                "服务器的路径搜索规则较新，当前客户端已关闭路径搜索，主搜索不受影响。请更新客户端。".into()
            }
            (Self::PathSearchNeedsUpgrade, false) => {
                "The server uses newer path search rules; path search is off and main search still works. Please update the client.".into()
            }
            (Self::Migrating { percent }, true) => format!(
                "服务器正在升级音效库（{}%），暂时使用本地缓存。",
                percent.map_or("?".into(), |p| p.to_string())
            ),
            (Self::Migrating { percent }, false) => format!(
                "The server is upgrading its sound library ({}%); using the local cache for now.",
                percent.map_or("?".into(), |p| p.to_string())
            ),
            (Self::Incompatible { server_version }, true) => format!(
                "服务器版本较新（{}），当前客户端无法读取它的音效库。请更新客户端。",
                server_version.as_deref().unwrap_or("未知")
            ),
            (Self::Incompatible { server_version }, false) => format!(
                "The server is newer ({}); this client cannot read its sound library. Please update the client.",
                server_version.as_deref().unwrap_or("unknown")
            ),
        }
    }
}

/// §7.2 explanation shown only when the user clicks a hidden feature on an old server.
pub fn old_server_explanation(chinese: bool) -> &'static str {
    if chinese {
        "服务器版本较旧，部分新功能（路径搜索、厂商资料导入）不可用。"
    } else {
        "The server is older; some new features (path search, vendor metadata import) are unavailable."
    }
}

/// Outcome for one server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerUpgradeStatus {
    pub path_search: PathSearch,
    /// Main search stays available unless the server is incompatible.
    pub main_search: bool,
    /// Vendor import limits, only when offered and this request may manage.
    pub vendor_import: Option<VendorImportInfo>,
    pub migration: Option<MigrationProgress>,
    pub notices: Vec<Notice>,
}

impl ServerUpgradeStatus {
    pub fn incompatible(&self) -> bool {
        self.notices
            .iter()
            .any(|notice| matches!(notice, Notice::Incompatible { .. }))
    }
}

fn percent(completed: Option<u64>, total: Option<u64>) -> Option<u8> {
    match (completed, total) {
        (Some(completed), Some(total)) if total > 0 => {
            Some((completed.min(total).saturating_mul(100) / total) as u8)
        }
        _ => None,
    }
}

/// Decide notices and feature availability (§6, §7, VN-04/06/10/12).
pub fn evaluate(input: &ServerUpgradeInput) -> ServerUpgradeStatus {
    let mut notices = Vec::new();
    let served: Vec<&str> = input
        .library_schema
        .iter()
        .flat_map(|schema| schema.snapshot_formats_served.iter())
        .map(|id| id.trim_start_matches("...").trim())
        .filter(|id| !id.is_empty())
        .collect();
    let supports: Vec<FormatSupport> = served.iter().map(|id| format_support(id)).collect();
    let incompatible = (!supports.is_empty()
        && supports
            .iter()
            .all(|support| *support == FormatSupport::Incompatible))
        || input
            .observed_formats
            .iter()
            .any(|id| format_support(id) == FormatSupport::Incompatible);

    let feature = input.features.get(SEARCH_PATH_FEATURE).copied();
    let newer_rules = feature.is_some_and(|rev| rev > SEARCH_PATH_REVISION)
        || input
            .observed_formats
            .iter()
            .any(|id| format_support(id) == FormatSupport::NewerPathRules)
        || (!supports.is_empty()
            && supports.contains(&FormatSupport::NewerPathRules)
            && !supports.contains(&FormatSupport::KnownPathSearch));
    let consistent_sp1 = !input.observed_formats.is_empty()
        && input
            .observed_formats
            .iter()
            .all(|id| id == PLAN_B_V9_FORMAT);
    let path_search = if incompatible {
        PathSearch::NotOffered
    } else if newer_rules {
        PathSearch::NeedsClientUpgrade
    } else if input.capabilities_known && feature == Some(SEARCH_PATH_REVISION) && consistent_sp1 {
        PathSearch::Available
    } else {
        PathSearch::NotOffered
    };

    let migration = {
        let capability = input
            .schema_migration
            .as_ref()
            .map(|migration| migration.state);
        if let Some(admin) = input
            .admin_migration
            .as_ref()
            .filter(|m| m.state.in_progress())
        {
            Some(MigrationProgress {
                state: admin.state,
                phase: admin.phase.clone(),
                percent: percent(admin.completed, admin.total),
            })
        } else if capability.is_some_and(MigrationState::in_progress)
            || input.health_upgrade.is_some()
        {
            let health = input.health_upgrade.clone().unwrap_or_default();
            Some(MigrationProgress {
                state: capability.unwrap_or(MigrationState::Running),
                phase: health.phase,
                percent: percent(health.completed, health.total),
            })
        } else {
            None
        }
    };

    if incompatible {
        notices.push(Notice::Incompatible {
            server_version: input.server_version.clone(),
        });
    } else {
        if let Some(migration) = &migration {
            notices.push(Notice::Migrating {
                percent: migration.percent,
            });
        }
        if path_search == PathSearch::NeedsClientUpgrade {
            notices.push(Notice::PathSearchNeedsUpgrade);
        }
        if let Some(upgrade) = &input.upgrade_available {
            notices.push(Notice::ClientUpgradeAvailable {
                version: upgrade.version.clone(),
            });
        }
    }
    let vendor_import = input
        .vendor_import
        .clone()
        .filter(|_| input.can_manage && !incompatible)
        .filter(|_| input.features.get(VENDOR_METADATA_FEATURE) == Some(&1));
    ServerUpgradeStatus {
        path_search,
        main_search: !incompatible,
        vendor_import,
        migration,
        notices,
    }
}

/// Session memory: each notice key is shown once per session (VN-06).
#[derive(Clone, Debug, Default)]
pub struct NoticeSession {
    shown: std::collections::BTreeSet<&'static str>,
}

impl NoticeSession {
    /// Notices not shown yet this session; marks them shown. Migration progress is
    /// always returned so the status bar can update its percentage.
    pub fn take<'a>(&mut self, status: &'a ServerUpgradeStatus) -> Vec<&'a Notice> {
        status
            .notices
            .iter()
            .filter(|notice| {
                matches!(notice, Notice::Migrating { .. }) || self.shown.insert(notice.key())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v9_input(served: &[&str], observed: &[&str]) -> ServerUpgradeInput {
        ServerUpgradeInput {
            server_version: Some("0.9.0".into()),
            capabilities_known: true,
            features: BTreeMap::from([(SEARCH_PATH_FEATURE.into(), 1)]),
            library_schema: Some(LibrarySchemaInfo {
                current: Some(9),
                source_search_rev: Some(1),
                snapshot_formats_served: served.iter().map(|s| s.to_string()).collect(),
            }),
            observed_formats: observed.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn upgrade_header_parsing() {
        assert_eq!(
            parse_upgrade_available(" 0.9.2 "),
            Some(UpgradeAvailable {
                version: Some("0.9.2".into())
            })
        );
        assert_eq!(
            parse_upgrade_available("v1.0.0-beta.1")
                .unwrap()
                .version
                .as_deref(),
            Some("1.0.0-beta.1")
        );
        assert_eq!(parse_upgrade_available("yes").unwrap().version, None);
        assert_eq!(parse_upgrade_available(""), None);
        assert_eq!(parse_upgrade_available("1.0 x"), None);
        assert_eq!(parse_upgrade_available(&"9".repeat(65)), None);
    }

    #[test]
    fn format_ids_are_classified() {
        assert_eq!(
            parse_format_id(PLAN_B_V9_FORMAT),
            Some(FormatId {
                schema: 9,
                tokenization: 3,
                search_path: Some(1)
            })
        );
        assert_eq!(
            format_support(compatibility::PLAN_B_FORMAT),
            FormatSupport::Readable
        );
        assert_eq!(
            format_support(PLAN_B_V9_FORMAT),
            FormatSupport::KnownPathSearch
        );
        assert_eq!(
            format_support("ale-s9-cld1-t3-m2-p1-a1-sp2"),
            FormatSupport::NewerPathRules
        );
        for unknown in [
            "ale-s10-cld1-t3-m2-p1-a1-sp1",
            "ale-s9-cld1-t4-m2-p1-a1-sp1",
            "ale-s9-cld1-t3-m2-p1-a1-sp1-sp2",
            "xyz-s9",
            "ale-s9--t3",
        ] {
            assert_eq!(
                format_support(unknown),
                FormatSupport::Incompatible,
                "{unknown}"
            );
        }
    }

    #[test]
    fn sp1_server_offers_path_search() {
        let status = evaluate(&v9_input(
            &[PLAN_B_V9_FORMAT, compatibility::PLAN_B_FORMAT],
            &[PLAN_B_V9_FORMAT],
        ));
        assert_eq!(status.path_search, PathSearch::Available);
        assert!(status.main_search);
        assert!(status.notices.is_empty());
    }

    #[test]
    fn sp2_disables_path_search_but_keeps_main_search() {
        let sp2 = "ale-s9-cld1-t3-m2-p1-a1-sp2";
        let status = evaluate(&v9_input(&[sp2, compatibility::PLAN_B_FORMAT], &[]));
        assert_eq!(status.path_search, PathSearch::NeedsClientUpgrade);
        assert!(status.main_search);
        assert_eq!(status.notices, vec![Notice::PathSearchNeedsUpgrade]);
        assert!(status.notices[0].offers_update_check());

        // Header says sp1 but the content descriptor says sp2 (VN-12 mismatch).
        let status = evaluate(&v9_input(&[PLAN_B_V9_FORMAT], &[PLAN_B_V9_FORMAT, sp2]));
        assert_eq!(status.path_search, PathSearch::NeedsClientUpgrade);
        assert!(status.main_search);

        // A newer `search_path` feature revision alone also disables it.
        let mut input = v9_input(&[PLAN_B_V9_FORMAT], &[PLAN_B_V9_FORMAT]);
        input.features.insert(SEARCH_PATH_FEATURE.into(), 2);
        assert_eq!(evaluate(&input).path_search, PathSearch::NeedsClientUpgrade);
    }

    #[test]
    fn unknown_major_is_incompatible() {
        let status = evaluate(&v9_input(&["ale-s10-cld1-t4-m2-p1-a1-sp1"], &[]));
        assert!(status.incompatible());
        assert!(!status.main_search);
        assert_eq!(status.path_search, PathSearch::NotOffered);
        assert_eq!(
            status.notices[0].text(true),
            "服务器版本较新（0.9.0），当前客户端无法读取它的音效库。请更新客户端。"
        );
        // An observed tampered format ID is incompatible as well.
        let status = evaluate(&v9_input(
            &[compatibility::PLAN_B_FORMAT],
            &["ale-s9-cld1-t3-m2-p1-a1-spX"],
        ));
        assert!(status.incompatible());
    }

    #[test]
    fn old_server_hides_features_without_prompt() {
        let status = evaluate(&ServerUpgradeInput::default());
        assert_eq!(status.path_search, PathSearch::NotOffered);
        assert!(status.main_search);
        assert!(status.notices.is_empty());
        assert!(status.vendor_import.is_none());
        let status = evaluate(&v9_input(
            &[compatibility::PLAN_B_FORMAT],
            &[compatibility::PLAN_B_FORMAT],
        ));
        assert_eq!(status.path_search, PathSearch::NotOffered);
        assert!(status.notices.is_empty());
    }

    #[test]
    fn migrating_server_reports_progress() {
        let mut input = v9_input(
            &[compatibility::PLAN_B_FORMAT],
            &[compatibility::PLAN_B_FORMAT],
        );
        input.schema_migration = Some(SchemaMigrationInfo {
            state: MigrationState::Running,
            endpoint: Some("/admin/api/migration".into()),
        });
        input.health_upgrade = Some(HealthUpgrade {
            phase: Some("backfill".into()),
            completed: Some(250),
            total: Some(1000),
        });
        let status = evaluate(&input);
        assert!(status.main_search);
        assert_eq!(status.path_search, PathSearch::NotOffered);
        assert_eq!(status.migration.as_ref().unwrap().percent, Some(25));
        assert_eq!(
            status.notices,
            vec![Notice::Migrating { percent: Some(25) }]
        );
        assert_eq!(
            status.notices[0].text(true),
            "服务器正在升级音效库（25%），暂时使用本地缓存。"
        );
        assert!(!status.notices[0].offers_update_check());

        input.admin_migration = Some(MigrationStatus {
            state: MigrationState::Paused,
            completed: Some(9),
            total: Some(10),
            rollback_backup: Some("secret-path".into()),
            ..Default::default()
        });
        let status = evaluate(&input);
        assert_eq!(status.migration.as_ref().unwrap().percent, Some(90));
        let json = serde_json::to_string(input.admin_migration.as_ref().unwrap()).unwrap();
        assert!(!json.contains("secret-path"));

        input.schema_migration.as_mut().unwrap().state = MigrationState::Done;
        input.health_upgrade = None;
        input.admin_migration = None;
        assert!(evaluate(&input).migration.is_none());
    }

    #[test]
    fn upgrade_header_prompts_once_per_session() {
        let mut input = v9_input(&[compatibility::PLAN_B_FORMAT], &[]);
        input.upgrade_available = parse_upgrade_available("0.9.0");
        let status = evaluate(&input);
        assert_eq!(
            status.notices[0].text(true),
            "服务器已升级，更新客户端可使用路径搜索和厂商资料"
        );
        assert!(status.notices[0].offers_update_check());
        let mut session = NoticeSession::default();
        assert_eq!(session.take(&status).len(), 1);
        assert!(session.take(&status).is_empty());
    }

    #[test]
    fn vendor_import_needs_feature_and_manage() {
        let limits = VendorImportInfo {
            revision: 1,
            max_file_bytes: 52_428_800,
            max_rows: 200_000,
        };
        let mut input = v9_input(&[PLAN_B_V9_FORMAT], &[PLAN_B_V9_FORMAT]);
        input.vendor_import = Some(limits.clone());
        assert!(evaluate(&input).vendor_import.is_none());
        input.can_manage = true;
        assert!(evaluate(&input).vendor_import.is_none());
        input.features.insert(VENDOR_METADATA_FEATURE.into(), 1);
        assert_eq!(evaluate(&input).vendor_import, Some(limits));
    }

    #[test]
    fn capability_fields_deserialize_with_unknown_values() {
        let migration: SchemaMigrationInfo =
            serde_json::from_str(r#"{"state":"rebuilding","endpoint":"/admin/api/migration"}"#)
                .unwrap();
        assert_eq!(migration.state, MigrationState::Unknown);
        let health: HealthUpgrade =
            serde_json::from_str(r#"{"phase":"copying","completed":1,"total":4}"#).unwrap();
        assert_eq!(percent(health.completed, health.total), Some(25));
    }
}
