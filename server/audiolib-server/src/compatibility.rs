// server/audiolib-server/src/compatibility.rs
//! Negotiate snapshot reads and business writes against the actual library format.
use crate::ApiError;
use audiolib_core::compatibility::{
    ACCEPT_FORMATS_HEADER, CLIENT_COMPATIBILITY_HEADER, CompatibilityManifest,
    LIBRARY_FORMAT_HEADER, MAX_COMPATIBILITY_HEADER_BYTES, PEER_CATALOG_INVALID,
    PEER_UPGRADE_REQUIRED, PeerRejection, SEARCH_STORAGE_MAX, SEARCH_STORAGE_MAX_HEADER,
    SERVER_COMPATIBILITY_HEADER, SNAPSHOT_FORMAT_HEADER, SNAPSHOT_SCHEMA_MAX,
    SNAPSHOT_SCHEMA_MAX_HEADER, SnapshotFormat, TOKENIZATION_MAX, TOKENIZATION_MAX_HEADER,
    check_peer, compatibility_manifest,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};

fn unsupported(format: &SnapshotFormat) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "SNAPSHOT_FORMAT_UNSUPPORTED",
        format!(
            "Snapshot requires format {}; update the client or its format declaration",
            format.id
        ),
    )
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    (values.next().is_none() && value.len() <= MAX_COMPATIBILITY_HEADER_BYTES).then_some(value)
}

/// Enforce exact format support and both application ranges for schema 6.
pub(crate) fn admit(headers: &HeaderMap, format: &SnapshotFormat) -> Result<(), ApiError> {
    if format.format.schema_version < 6 {
        return Ok(());
    }
    let accepted = single(headers, ACCEPT_FORMATS_HEADER).ok_or_else(|| unsupported(format))?;
    if !accepted.split(',').map(str::trim).any(|id| id == format.id) {
        return Err(unsupported(format));
    }
    for (header, required) in [
        (SNAPSHOT_SCHEMA_MAX_HEADER, format.format.schema_version),
        (SEARCH_STORAGE_MAX_HEADER, SEARCH_STORAGE_MAX),
        (TOKENIZATION_MAX_HEADER, TOKENIZATION_MAX),
    ] {
        if single(headers, header)
            .and_then(|value| value.parse::<u32>().ok())
            .is_none_or(|maximum| maximum < required)
        {
            return Err(unsupported(format));
        }
    }
    let peer = admit_peer(headers)?;
    if !format.is_readable_by(&peer) {
        return Err(unsupported(format));
    }
    Ok(())
}

/// Translate a marker refusal into the stable 409 reply (Compatibility_Markers.md §4).
fn rejection(reason: PeerRejection, peer_version: &str) -> ApiError {
    let message = match reason {
        PeerRejection::BlockedVersion => format!(
            "Client version {peer_version} is blocked by this server; install a different client release"
        ),
        PeerRejection::PeerUpgradeRequired => {
            "The client predates compatibility markers (0.7.2); upgrade the client".to_owned()
        }
        PeerRejection::ProtocolPeerTooOld => format!(
            "Client {peer_version} only speaks an older API protocol than this server; upgrade the client"
        ),
        PeerRejection::ProtocolPeerTooNew => format!(
            "Client {peer_version} requires a newer API protocol than this server supports; upgrade the server"
        ),
        PeerRejection::Inconsistent => {
            "The client compatibility catalog is inconsistent; upgrade the client".to_owned()
        }
    };
    let error = ApiError::new(StatusCode::CONFLICT, reason.error_code(), message);
    if reason == PeerRejection::ProtocolPeerTooNew {
        // Hint: this side, not the client, must be upgraded.
        error.with_sub_code("SERVER_UPGRADE_REQUIRED")
    } else {
        error
    }
}

/// Marker check of the client catalog. Client `features` are logged for diagnostics
/// only and never change admission or the response.
fn admit_peer(headers: &HeaderMap) -> Result<CompatibilityManifest, ApiError> {
    if !headers.contains_key(CLIENT_COMPATIBILITY_HEADER) {
        // No catalog at all: a pre-marker (0.4.x/0.5.x or JSON-only) caller.
        return Err(rejection(PeerRejection::PeerUpgradeRequired, ""));
    }
    let peer: CompatibilityManifest = single(headers, CLIENT_COMPATIBILITY_HEADER)
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                PEER_CATALOG_INVALID,
                "The client compatibility catalog is malformed, duplicated or too large; upgrade the client",
            )
        })?;
    tracing::debug!(
        client_version = %peer.application_version,
        client_marker_revision = ?peer.marker_revision,
        client_features = ?peer.features,
        "client compatibility catalog"
    );
    check_peer(
        &compatibility_manifest(),
        &peer.application_version,
        Some(&peer),
    )
    .map_err(|reason| rejection(reason, &peer.application_version))?;
    Ok(peer)
}

/// Settings, cache jobs and the playback no-op permit legacy JSON callers to omit
/// the catalog (the only no-header exemption). A supplied malformed or incompatible catalog is always refused.
pub(crate) fn admit_mutation(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(CLIENT_COMPATIBILITY_HEADER) {
        admit_peer(headers)?;
    }
    Ok(())
}

/// A library write can return current AudioFile values, so the peer must explicitly
/// understand the complete actual format. Legacy JSON admission only applies to
/// settings, cache jobs and the deprecated playback no-op, never business writes.
pub(crate) fn admit_library_mutation(
    headers: &HeaderMap,
    format: &SnapshotFormat,
) -> Result<(), ApiError> {
    let peer = admit_peer(headers)?;
    if !format.is_readable_by(&peer) {
        return Err(unsupported(format));
    }
    Ok(())
}

/// Attach the actual prepared format, including on 304 replies.
pub(crate) fn response_headers(
    headers: &mut HeaderMap,
    format: &SnapshotFormat,
) -> Result<(), ApiError> {
    let catalog = serde_json::to_string(&compatibility_manifest())
        .map_err(|_| ApiError::internal("Cannot serialize compatibility catalog"))?;
    for (name, value) in [
        (SNAPSHOT_FORMAT_HEADER, format.id.clone()),
        (
            LIBRARY_FORMAT_HEADER,
            serde_json::to_string(&format.format)
                .map_err(|_| ApiError::internal("Cannot serialize library format"))?,
        ),
        (SERVER_COMPATIBILITY_HEADER, catalog),
        ("x-schema-version", format.format.schema_version.to_string()),
        (
            "x-search-storage-revision",
            format
                .format
                .features
                .get("search_storage")
                .copied()
                .unwrap_or(0)
                .to_string(),
        ),
        (
            "x-tokenization-revision",
            format
                .format
                .features
                .get("tokenization")
                .copied()
                .unwrap_or(0)
                .to_string(),
        ),
        (
            "x-source-search-revision",
            format
                .format
                .source_search_revision
                .unwrap_or(0)
                .to_string(),
        ),
        (
            "x-source-tags-revision",
            format.format.source_tags_revision.unwrap_or(0).to_string(),
        ),
    ] {
        headers.insert(
            name,
            HeaderValue::from_str(&value)
                .map_err(|_| ApiError::internal("Invalid compatibility header"))?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::compatibility::{LibraryFormat, PLAN_B_FORMAT};

    fn fixture() -> (HeaderMap, SnapshotFormat) {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT_FORMATS_HEADER,
            HeaderValue::from_static(PLAN_B_FORMAT),
        );
        headers.insert(
            CLIENT_COMPATIBILITY_HEADER,
            HeaderValue::from_str(&serde_json::to_string(&compatibility_manifest()).unwrap())
                .unwrap(),
        );
        for (name, value) in [
            (SNAPSHOT_SCHEMA_MAX_HEADER, SNAPSHOT_SCHEMA_MAX),
            (SEARCH_STORAGE_MAX_HEADER, SEARCH_STORAGE_MAX),
            (TOKENIZATION_MAX_HEADER, TOKENIZATION_MAX),
        ] {
            headers.insert(name, HeaderValue::from(value));
        }
        (
            headers,
            SnapshotFormat {
                id: PLAN_B_FORMAT.into(),
                format: LibraryFormat::target(),
            },
        )
    }

    #[test]
    fn exact_format_and_mutual_versions_are_required_independently() {
        let (headers, format) = fixture();
        admit(&headers, &format).unwrap();
        assert!(admit(&HeaderMap::new(), &format).is_err());
        for name in [
            ACCEPT_FORMATS_HEADER,
            CLIENT_COMPATIBILITY_HEADER,
            TOKENIZATION_MAX_HEADER,
        ] {
            let mut missing = headers.clone();
            missing.remove(name);
            assert!(admit(&missing, &format).is_err(), "{name}");
        }
        let mut one_way = headers.clone();
        let mut peer = compatibility_manifest();
        peer.marker_revision = None;
        one_way.insert(
            CLIENT_COMPATIBILITY_HEADER,
            HeaderValue::from_str(&serde_json::to_string(&peer).unwrap()).unwrap(),
        );
        assert!(admit(&one_way, &format).is_err());
        let mut unknown = headers.clone();
        unknown.insert(
            ACCEPT_FORMATS_HEADER,
            HeaderValue::from_static("ale-s6-unknown"),
        );
        assert!(admit(&unknown, &format).is_err());
        let mut duplicate = headers.clone();
        duplicate.append(
            ACCEPT_FORMATS_HEADER,
            HeaderValue::from_static(PLAN_B_FORMAT),
        );
        assert!(admit(&duplicate, &format).is_err());
    }

    #[test]
    fn library_writes_require_catalog_and_complete_actual_format() {
        let (mut headers, format) = fixture();
        // Snapshot-transfer headers do not substitute for or constrain the catalog
        // used by a JSON mutation. The full actual library descriptor still does.
        headers.remove(ACCEPT_FORMATS_HEADER);
        headers.remove(SNAPSHOT_SCHEMA_MAX_HEADER);
        admit_library_mutation(&headers, &format).unwrap();
        assert!(admit_library_mutation(&HeaderMap::new(), &format).is_err());
        let mut old = compatibility_manifest();
        old.snapshot_read_formats =
            vec![audiolib_core::compatibility::PLAN_B_INITIAL_FORMAT.into()];
        headers.insert(
            CLIENT_COMPATIBILITY_HEADER,
            HeaderValue::from_str(&serde_json::to_string(&old).unwrap()).unwrap(),
        );
        assert!(admit_library_mutation(&headers, &format).is_err());
        let (headers, mut unknown) = fixture();
        unknown.format.features.insert("future_feature".into(), 1);
        assert!(admit_library_mutation(&headers, &unknown).is_err());
    }

    fn with_catalog(headers: &HeaderMap, catalog: &CompatibilityManifest) -> HeaderMap {
        let mut headers = headers.clone();
        headers.insert(
            CLIENT_COMPATIBILITY_HEADER,
            HeaderValue::from_str(&serde_json::to_string(catalog).unwrap()).unwrap(),
        );
        headers
    }

    fn code(result: Result<(), ApiError>) -> (&'static str, Option<&'static str>) {
        let error = result.unwrap_err();
        (error.code(), error.sub_code())
    }

    #[test]
    fn marker_peers_of_any_version_are_admitted() {
        let (headers, format) = fixture();
        for version in ["0.7.2", "0.8.0", "9.0.0"] {
            let mut peer = compatibility_manifest();
            peer.application_version = version.into();
            peer.catalog_revision += 5;
            peer.peer_application_ranges = vec!["<0.1.0".into()];
            let headers = with_catalog(&headers, &peer);
            admit(&headers, &format).unwrap();
            admit_mutation(&headers).unwrap();
            admit_library_mutation(&headers, &format).unwrap();
        }
    }

    #[test]
    fn unmarked_and_missing_catalogs_require_peer_upgrade() {
        let (headers, format) = fixture();
        let mut old = compatibility_manifest();
        old.application_version = "0.7.1".into();
        old.marker_revision = None;
        old.protocol = None;
        let old_headers = with_catalog(&headers, &old);
        for result in [
            admit(&old_headers, &format),
            admit_mutation(&old_headers),
            admit_library_mutation(&old_headers, &format),
        ] {
            assert_eq!(code(result), (PEER_UPGRADE_REQUIRED, None));
        }
        let mut missing = headers.clone();
        missing.remove(CLIENT_COMPATIBILITY_HEADER);
        assert_eq!(
            code(admit_library_mutation(&missing, &format)),
            (PEER_UPGRADE_REQUIRED, None)
        );
        assert_eq!(
            code(admit(&missing, &format)),
            (PEER_UPGRADE_REQUIRED, None)
        );
    }

    #[test]
    fn protocol_direction_selects_the_side_to_upgrade() {
        let (headers, format) = fixture();
        let mut newer = compatibility_manifest();
        newer.protocol = Some(audiolib_core::compatibility::ProtocolRange { min: 2, max: 3 });
        newer.api_protocols = vec!["api/v2".into(), "api/v3".into()];
        let newer = with_catalog(&headers, &newer);
        assert_eq!(
            code(admit_library_mutation(&newer, &format)),
            (
                audiolib_core::compatibility::PROTOCOL_UNSUPPORTED,
                Some("SERVER_UPGRADE_REQUIRED")
            )
        );
        assert_eq!(
            code(admit_mutation(&newer)).0,
            audiolib_core::compatibility::PROTOCOL_UNSUPPORTED
        );
        let mut older = compatibility_manifest();
        older.protocol = Some(audiolib_core::compatibility::ProtocolRange { min: 0, max: 0 });
        older.api_protocols = vec!["api/v0".into()];
        let older = with_catalog(&headers, &older);
        assert_eq!(code(admit(&older, &format)), (PEER_UPGRADE_REQUIRED, None));
    }

    #[test]
    fn blocked_version_maps_to_peer_version_blocked() {
        assert_eq!(
            rejection(PeerRejection::BlockedVersion, "0.7.3").code(),
            audiolib_core::compatibility::PEER_VERSION_BLOCKED
        );
        assert_eq!(
            rejection(PeerRejection::BlockedVersion, "0.7.3").status(),
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn header_exemption_is_limited_to_settings_and_cache_tasks() {
        let (_, format) = fixture();
        admit_mutation(&HeaderMap::new()).unwrap();
        assert_eq!(
            code(admit_library_mutation(&HeaderMap::new(), &format)).0,
            PEER_UPGRADE_REQUIRED
        );
        let mut malformed = HeaderMap::new();
        malformed.insert(CLIENT_COMPATIBILITY_HEADER, HeaderValue::from_static("{"));
        assert_eq!(code(admit_mutation(&malformed)).0, PEER_CATALOG_INVALID);
        // check_peer compares against the catalog's own version, so force the
        // Inconsistent mapping directly.
        assert_eq!(
            rejection(PeerRejection::Inconsistent, "0.9.9").code(),
            PEER_CATALOG_INVALID
        );
    }

    #[test]
    fn schema_max_headers_follow_core_constants() {
        let (headers, format) = fixture();
        for name in [
            SNAPSHOT_SCHEMA_MAX_HEADER,
            SEARCH_STORAGE_MAX_HEADER,
            TOKENIZATION_MAX_HEADER,
        ] {
            let mut low = headers.clone();
            low.insert(name, HeaderValue::from_static("0"));
            assert!(admit(&low, &format).is_err(), "{name}");
        }
    }
}
