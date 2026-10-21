// server/audiolib-server/src/compatibility.rs
//! Snapshot negotiation happens before copying/compressing or streaming the library.
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
                "A valid client compatibility catalog is required",
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
            "Client and server application compatibility ranges do not agree",
        ));
    }
    Ok(peer)
}

/// Legacy JSON callers may omit the catalog. A supplied catalog is always checked;
/// malformed, duplicate or incompatible declarations never fall back to legacy mode.
pub(crate) fn admit_mutation(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(CLIENT_COMPATIBILITY_HEADER) {
        admit_peer(headers)?;
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
        peer.peer_application_ranges = vec!["<0.4.0".into()];
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
}
