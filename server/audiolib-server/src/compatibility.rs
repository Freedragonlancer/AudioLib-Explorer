// server/audiolib-server/src/compatibility.rs
//! Negotiate snapshot reads and business writes against the actual library format.
use crate::ApiError;
use audiolib_core::compatibility::{
    ACCEPT_FORMATS_HEADER, CLIENT_COMPATIBILITY_HEADER, CompatibilityManifest,
    LIBRARY_FORMAT_HEADER, MAX_COMPATIBILITY_HEADER_BYTES, SERVER_COMPATIBILITY_HEADER,
    SNAPSHOT_FORMAT_HEADER, SnapshotFormat, compatibility_manifest, peer_versions_compatible,
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
        (
            "x-audiolib-snapshot-schema-max",
            format.format.schema_version,
        ),
        ("x-audiolib-search-storage-max", 1),
        ("x-audiolib-tokenization-max", 3),
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

/// Validate a modern peer at request admission, including after a client preflight.
fn admit_peer(headers: &HeaderMap) -> Result<CompatibilityManifest, ApiError> {
    let peer: CompatibilityManifest = single(headers, CLIENT_COMPATIBILITY_HEADER)
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "PEER_VERSION_UNSUPPORTED",
                "A valid client compatibility catalog is required; upgrade the client before writing",
            )
        })?;
    if !peer_versions_compatible(
        &compatibility_manifest(),
        &peer.application_version,
        Some(&peer),
    ) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "PEER_VERSION_UNSUPPORTED",
            "Client and server application compatibility ranges do not agree; upgrade the client",
        ));
    }
    Ok(peer)
}

/// Settings, cache jobs and the playback no-op permit legacy JSON callers to omit
/// the catalog. A supplied malformed or incompatible catalog is always refused.
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
            ("x-audiolib-snapshot-schema-max", "6"),
            ("x-audiolib-search-storage-max", "1"),
            ("x-audiolib-tokenization-max", "3"),
        ] {
            headers.insert(name, HeaderValue::from_static(value));
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
            "x-audiolib-tokenization-max",
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
        headers.remove("x-audiolib-snapshot-schema-max");
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
}
