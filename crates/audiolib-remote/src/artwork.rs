// crates/audiolib-remote/src/artwork.rs
//! Optional embedded-cover HTTP reads. Images never enter the snapshot or audio stream.
use crate::{Capabilities, RemoteClient, RemoteError, Result};
use std::{sync::atomic::AtomicBool, time::Duration};

/// Largest encoded cover accepted from a server, matching the bounded image decoder.
pub const MAX_ARTWORK_BYTES: u64 = 8 * 1024 * 1024;
const ARTWORK_TIMEOUT: Duration = Duration::from_secs(30);

/// An optional image response; decoding remains on the caller's image worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtworkReply {
    /// The caller's ETag still identifies the current image.
    NotModified,
    /// No image or an older server without the endpoint (`404`/`405`).
    Missing,
    /// A bounded JPEG/PNG response with its opaque entity tag.
    Data {
        bytes: Vec<u8>,
        etag: Option<String>,
        mime: String,
    },
}

impl Capabilities {
    /// A shared album representation is separate from the historical WAV endpoint.
    pub fn album_entity_artwork_usable(&self) -> bool {
        self.supported
            && self.compatibility.as_ref().is_some_and(|manifest| {
                manifest
                    .features
                    .get(audiolib_core::compatibility::ALBUM_ENTITY_ARTWORK_FEATURE)
                    == Some(&audiolib_core::compatibility::ALBUM_ENTITY_ARTWORK_REVISION)
            })
    }

    /// Covers are opt-in: an absent `album_artwork` marker disables every image request.
    pub fn album_artwork_usable(&self) -> bool {
        self.supported
            && self.compatibility.as_ref().is_some_and(|manifest| {
                manifest
                    .features
                    .get(audiolib_core::compatibility::ALBUM_ARTWORK_FEATURE)
                    .is_some_and(|revision| *revision >= 1)
            })
    }
}

impl RemoteClient {
    /// Read a cover only after [`Capabilities::album_artwork_usable`] is true.
    /// This unbound form supports older artwork endpoints. Snapshot readers use
    /// [`Self::artwork_for_library`]; current servers require that library binding.
    ///
    /// `side` bounds the long side to `1..=512`; omission requests the embedded image.
    /// Cancellation closes the request socket, including while a sleeping root opens.
    /// The caller must use a bounded decoder before displaying or caching pixels.
    /// Snapshot consumers use [`Self::artwork_for_library`]; identity-bound servers
    /// reject this older, unbound transport entry point.
    pub fn artwork(
        &self,
        id: i64,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<ArtworkReply> {
        self.artwork_request(None, None, id, side, if_none_match, cancel)
    }

    /// Read a snapshot record's cover with an atomic server-side library identity check.
    /// Successful responses must repeat that identity, including conditional `304`s.
    pub fn artwork_for_library(
        &self,
        library_id: &str,
        id: i64,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<ArtworkReply> {
        if library_id.trim().is_empty() || ureq::http::HeaderValue::from_str(library_id).is_err() {
            return Err(RemoteError::Validation(
                "Invalid cached library identity".into(),
            ));
        }
        self.artwork_request(Some(library_id), None, id, side, if_none_match, cancel)
    }

    /// Read a server-authoritative album and verify its library, namespace and ID.
    #[allow(clippy::too_many_arguments)]
    pub fn album_artwork_for_library(
        &self,
        library_id: &str,
        expected_namespace: &str,
        album_id: i64,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<ArtworkReply> {
        if album_id <= 0
            || library_id.trim().is_empty()
            || expected_namespace.trim().is_empty()
            || ureq::http::HeaderValue::from_str(library_id).is_err()
            || ureq::http::HeaderValue::from_str(expected_namespace).is_err()
        {
            return Err(RemoteError::Validation(
                "Invalid album or library identity".into(),
            ));
        }
        self.artwork_request(
            Some(library_id),
            Some(expected_namespace),
            album_id,
            side,
            if_none_match,
            cancel,
        )
    }

    /// Use authoritative album IDs only for an original schema-8 server snapshot.
    /// Legacy projections use the same sorted whole-album member candidates on every
    /// selection, never a client-generated album ID or the currently selected WAV.
    #[allow(clippy::too_many_arguments)]
    pub fn album_artwork_for_snapshot(
        &self,
        state: &crate::CacheState,
        capabilities: &Capabilities,
        expected_namespace: &str,
        album_id: i64,
        member_ids: &[i64],
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<ArtworkReply> {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(RemoteError::Cancelled);
        }
        if member_ids.len() > audiolib_core::MAX_ALBUM_MEMBERS {
            return Err(RemoteError::Validation(
                "Too many album artwork candidates".into(),
            ));
        }
        let authoritative = state
            .format
            .as_ref()
            .and_then(|format| format.source.as_ref())
            .is_some_and(|format| {
                format.schema_version >= 8 && format.features.get("album_entities") == Some(&1)
            });
        if authoritative && capabilities.album_entity_artwork_usable() {
            return self.album_artwork_for_library(
                &state.library_id,
                expected_namespace,
                album_id,
                side,
                if_none_match,
                cancel,
            );
        }
        if !capabilities.album_artwork_usable() {
            return Ok(ArtworkReply::Missing);
        }
        let mut candidates = member_ids.to_vec();
        candidates.sort_unstable();
        candidates.dedup();
        candidates.truncate(audiolib_core::MAX_ALBUM_MEMBERS);
        for id in candidates {
            match self.artwork_for_library(&state.library_id, id, side, if_none_match, cancel)? {
                ArtworkReply::Missing => {}
                reply => return Ok(reply),
            }
        }
        Ok(ArtworkReply::Missing)
    }

    #[allow(clippy::too_many_arguments)]
    fn artwork_request(
        &self,
        library_id: Option<&str>,
        expected_namespace: Option<&str>,
        id: i64,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<ArtworkReply> {
        if side.is_some_and(|side| !(1..=512).contains(&side)) {
            return Err(RemoteError::Validation(
                "artwork side must be 1..=512".into(),
            ));
        }
        let collection = if expected_namespace.is_some() {
            "albums"
        } else {
            "audio"
        };
        let path = match side {
            Some(side) => format!("/{collection}/{id}/artwork?side={side}"),
            None => format!("/{collection}/{id}/artwork"),
        };
        let client = self.clone();
        let library_id = library_id.map(str::to_owned);
        let expected_namespace = expected_namespace.map(str::to_owned);
        let if_none_match = if_none_match.map(str::to_owned);
        crate::client::call_cancellable(
            move |abort| {
                let url = format!("{}{path}", client.api);
                let context = format!("GET {path}");
                let agent = crate::abort::abortable_agent(ARTWORK_TIMEOUT, ARTWORK_TIMEOUT, abort);
                let response = crate::abort::call_fresh(|| {
                    let mut request = agent.get(&url).header("Accept", "image/jpeg, image/png");
                    if !client.token.is_empty() {
                        request =
                            request.header("Authorization", &format!("Bearer {}", client.token));
                    }
                    if let Some(etag) = &if_none_match {
                        request = request.header("If-None-Match", etag);
                    }
                    if let Some(library_id) = &library_id {
                        request = request.header(crate::LIBRARY_ID_HEADER, library_id);
                    }
                    if let Some(namespace) = &expected_namespace {
                        request = request.header("x-audiolib-album-namespace", namespace);
                    }
                    request
                })
                .map_err(|error| RemoteError::from_transport(error, &url))?;
                if matches!(response.status().as_u16(), 404 | 405) {
                    return Ok(ArtworkReply::Missing);
                }
                if matches!(response.status().as_u16(), 200 | 304)
                    && let Some(expected) = &library_id
                {
                    let identities = response.headers().get_all("x-library-id");
                    let mut identities = identities.iter();
                    let identity = identities.next().and_then(|value| value.to_str().ok());
                    if identity != Some(expected.as_str()) || identities.next().is_some() {
                        return Err(RemoteError::LibraryChanged(
                            "Artwork does not belong to the opened snapshot; synchronize before displaying it".into(),
                        ));
                    }
                    if let Some(namespace) = &expected_namespace {
                        let mut identities = response.headers().get_all("x-album-id").iter();
                        let identity = identities.next().and_then(|value| value.to_str().ok());
                        let expected = id.to_string();
                        if identity != Some(expected.as_str()) || identities.next().is_some() {
                            return Err(RemoteError::LibraryChanged(
                                "Artwork does not belong to the requested album".into(),
                            ));
                        }
                        let mut namespaces = response.headers().get_all("x-album-namespace").iter();
                        let actual = namespaces.next().and_then(|value| value.to_str().ok());
                        if actual != Some(namespace.as_str()) || namespaces.next().is_some() {
                            return Err(RemoteError::LibraryChanged(
                                "Artwork does not belong to the opened album namespace".into(),
                            ));
                        }
                    }
                }
                let Some(mut response) =
                    client.checked_conditional(response, &context, if_none_match.is_some())?
                else {
                    return Ok(ArtworkReply::NotModified);
                };
                if response.status().as_u16() != 200 {
                    return Err(RemoteError::Protocol(format!(
                        "{context}: expected HTTP 200"
                    )));
                }
                let mime = response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                if !matches!(mime.as_str(), "image/png" | "image/jpeg") {
                    return Err(RemoteError::Protocol(format!(
                        "{context}: expected JPEG or PNG"
                    )));
                }
                let etag = response
                    .headers()
                    .get("etag")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let bytes = response
                    .body_mut()
                    .with_config()
                    .limit(MAX_ARTWORK_BYTES)
                    .read_to_vec()
                    .map_err(|error| RemoteError::from_transport(error, &url))?;
                let valid_magic = match mime.as_str() {
                    "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                    "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
                    _ => false,
                };
                if !valid_magic {
                    return Err(RemoteError::Protocol(format!(
                        "{context}: invalid image signature"
                    )));
                }
                Ok(ArtworkReply::Data { bytes, etag, mime })
            },
            cancel,
            &mut |_| {},
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{TestResponse, TestServer};
    use std::sync::{Arc, atomic::Ordering};

    #[test]
    fn optional_artwork_requires_its_explicit_feature() {
        assert!(!Capabilities::default().album_artwork_usable());
        assert!(!Capabilities::unknown().album_artwork_usable());
        let mut capability = Capabilities {
            compatibility: Some(audiolib_core::compatibility::compatibility_manifest()),
            ..Capabilities::default()
        };
        assert!(capability.album_artwork_usable());
        let features = &mut capability.compatibility.as_mut().unwrap().features;
        features.insert("album_artwork".into(), 2);
        assert!(capability.album_artwork_usable());
        capability
            .compatibility
            .as_mut()
            .unwrap()
            .features
            .remove("album_artwork");
        assert!(!capability.album_artwork_usable());
    }

    #[test]
    fn native_album_transport_binds_library_namespace_and_album_identity() {
        let server = TestServer::start(|request| {
            assert_eq!(request.path, "/api/v1/albums/11/artwork?side=40");
            assert_eq!(request.header("x-audiolib-library-id"), Some("library"));
            assert_eq!(
                request.header("x-audiolib-album-namespace"),
                Some("namespace")
            );
            let mut response = if request.header("if-none-match").is_some() {
                TestResponse::bytes(304, Vec::new())
            } else {
                TestResponse::bytes(200, b"\x89PNG\r\n\x1a\nfixture".to_vec())
            };
            response.content_type = "image/png";
            response
                .with_header("X-Library-Id", "library")
                .with_header("X-Album-Id", "11")
                .with_header("X-Album-Namespace", "namespace")
        });
        let client = RemoteClient::new(&server.url, "token").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            client
                .album_artwork_for_library("library", "namespace", 11, Some(40), None, &cancel)
                .unwrap(),
            ArtworkReply::Data { .. }
        ));
        assert_eq!(
            client
                .album_artwork_for_library(
                    "library",
                    "namespace",
                    11,
                    Some(40),
                    Some("\"cached\""),
                    &cancel
                )
                .unwrap(),
            ArtworkReply::NotModified
        );
        let format = audiolib_core::compatibility::LibraryFormat::target();
        let state = crate::CacheState {
            library_id: "library".into(),
            schema_version: 8,
            format: Some(Box::new(crate::CacheFormat {
                source: Some(format.clone()),
                local: format,
                generation: 0,
            })),
            ..Default::default()
        };
        let capabilities = Capabilities {
            compatibility: Some(audiolib_core::compatibility::compatibility_manifest()),
            ..Default::default()
        };
        assert!(matches!(
            client
                .album_artwork_for_snapshot(
                    &state,
                    &capabilities,
                    "namespace",
                    11,
                    &[999],
                    Some(40),
                    None,
                    &cancel
                )
                .unwrap(),
            ArtworkReply::Data { .. }
        ));
        for status in [200, 304] {
            for invalid in 0..5 {
                let bad = TestServer::start(move |_| {
                    let response = TestResponse::bytes(status, Vec::new())
                        .with_header("X-Library-Id", "library");
                    let response = match invalid {
                        0 => response.with_header("X-Album-Id", "12"),
                        1 => response
                            .with_header("X-Album-Id", "11")
                            .with_header("X-Album-Id", "11"),
                        _ => response.with_header("X-Album-Id", "11"),
                    };
                    match invalid {
                        2 => response,
                        3 => response.with_header("X-Album-Namespace", "different"),
                        4 => response
                            .with_header("X-Album-Namespace", "namespace")
                            .with_header("X-Album-Namespace", "namespace"),
                        _ => response.with_header("X-Album-Namespace", "namespace"),
                    }
                });
                assert!(matches!(
                    RemoteClient::new(&bad.url, "token")
                        .unwrap()
                        .album_artwork_for_library(
                            "library",
                            "namespace",
                            11,
                            None,
                            Some("\"cached\""),
                            &cancel
                        ),
                    Err(RemoteError::LibraryChanged(_))
                ));
            }
        }
    }

    #[test]
    fn legacy_snapshot_uses_fixed_members_not_locally_projected_album_ids() {
        let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = paths.clone();
        let server = TestServer::start(move |request| {
            assert_eq!(request.header("x-audiolib-album-namespace"), None);
            observed.lock().unwrap().push(request.path.clone());
            if request.path == "/api/v1/audio/1/artwork?side=40" {
                return TestResponse::bytes(404, Vec::new());
            }
            assert_eq!(request.path, "/api/v1/audio/2/artwork?side=40");
            let mut response = TestResponse::bytes(200, b"\x89PNG\r\n\x1a\nfixture".to_vec())
                .with_header("X-Library-Id", "library");
            response.content_type = "image/png";
            response
        });
        let local = audiolib_core::compatibility::LibraryFormat::target();
        let mut source = local.clone();
        source.schema_version = 7;
        source.features.remove("album_entities");
        source.features.remove("metadata_storage");
        let state = crate::CacheState {
            library_id: "library".into(),
            schema_version: 8,
            format: Some(Box::new(crate::CacheFormat {
                source: Some(source),
                local,
                generation: 1,
            })),
            ..Default::default()
        };
        let capabilities = Capabilities {
            compatibility: Some(audiolib_core::compatibility::compatibility_manifest()),
            ..Default::default()
        };
        let reply = RemoteClient::new(&server.url, "token")
            .unwrap()
            .album_artwork_for_snapshot(
                &state,
                &capabilities,
                "local-only-namespace",
                987654,
                &[9, 2, 1, 2],
                Some(40),
                None,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(matches!(reply, ArtworkReply::Data { .. }));
        assert_eq!(
            *paths.lock().unwrap(),
            [
                "/api/v1/audio/1/artwork?side=40",
                "/api/v1/audio/2/artwork?side=40"
            ]
        );
    }

    #[test]
    fn artwork_uses_authentication_thumbnail_and_conditional_etag() {
        let server = TestServer::start(|request| {
            assert_eq!(request.path, "/api/v1/audio/7/artwork?side=192");
            assert_eq!(request.header("authorization"), Some("Bearer test-token"));
            if request.header("if-none-match") == Some("\"cover-1\"") {
                return TestResponse::bytes(304, Vec::new());
            }
            let mut response = TestResponse::bytes(200, b"\x89PNG\r\n\x1a\nfixture".to_vec())
                .with_header("ETag", "\"cover-1\"");
            response.content_type = "image/png";
            response
        });
        let client = RemoteClient::new(&server.url, "test-token").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(
            matches!(client.artwork(7, Some(192), None, &cancel).unwrap(),
            ArtworkReply::Data { etag: Some(etag), .. } if etag == "\"cover-1\"")
        );
        assert_eq!(
            client
                .artwork(7, Some(192), Some("\"cover-1\""), &cancel)
                .unwrap(),
            ArtworkReply::NotModified
        );
    }

    #[test]
    fn snapshot_artwork_sends_and_verifies_library_identity_including_304() {
        let server = TestServer::start(|request| {
            assert_eq!(
                request.header("x-audiolib-library-id"),
                Some("displayed-library")
            );
            let mut response = if request.header("if-none-match").is_some() {
                TestResponse::bytes(304, Vec::new())
            } else {
                TestResponse::bytes(200, b"\x89PNG\r\n\x1a\nfixture".to_vec())
            }
            .with_header("X-Library-Id", "displayed-library");
            response.content_type = "image/png";
            response
        });
        let client = RemoteClient::new(&server.url, "token").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            client
                .artwork_for_library("displayed-library", 1, None, None, &cancel)
                .unwrap(),
            ArtworkReply::Data { .. }
        ));
        assert_eq!(
            client
                .artwork_for_library("displayed-library", 1, None, Some("\"cached\""), &cancel)
                .unwrap(),
            ArtworkReply::NotModified
        );
        for identity in ["", "\r\nspoof"] {
            assert!(matches!(
                client.artwork_for_library(identity, 1, None, None, &cancel),
                Err(RemoteError::Validation(_))
            ));
        }
    }

    #[test]
    fn snapshot_artwork_rejects_missing_mismatched_and_duplicate_reply_identity() {
        for case in 0..4 {
            let server = TestServer::start(move |_| {
                let status = if case == 1 { 304 } else { 200 };
                let mut response =
                    TestResponse::bytes(status, b"\x89PNG\r\n\x1a\nfixture".to_vec());
                response.content_type = "image/png";
                if case != 0 {
                    response = response.with_header(
                        "X-Library-Id",
                        if case == 3 {
                            "displayed-library"
                        } else {
                            "other-library"
                        },
                    );
                }
                if case == 3 {
                    response = response.with_header("X-Library-Id", "displayed-library");
                }
                response
            });
            let client = RemoteClient::new(&server.url, "token").unwrap();
            assert!(
                matches!(
                    client.artwork_for_library(
                        "displayed-library",
                        1,
                        None,
                        Some("\"cached\""),
                        &AtomicBool::new(false)
                    ),
                    Err(RemoteError::LibraryChanged(_))
                ),
                "case {case}"
            );
        }
    }

    #[test]
    fn absent_and_legacy_endpoints_degrade_but_authentication_errors_remain_errors() {
        for status in [404, 405, 401] {
            let server = TestServer::start(move |_| TestResponse::json(status, "{}"));
            let result = RemoteClient::new(&server.url, "token").unwrap().artwork(
                1,
                None,
                None,
                &AtomicBool::new(false),
            );
            if status == 401 {
                assert!(matches!(result, Err(RemoteError::Unauthorized(_))));
            } else {
                assert_eq!(result.unwrap(), ArtworkReply::Missing);
            }
        }
    }

    #[test]
    fn malformed_oversized_and_unconditional_304_are_rejected() {
        for case in 0..4 {
            let server = TestServer::start(move |_| {
                let mut response = match case {
                    0 => TestResponse::bytes(304, Vec::new()),
                    1 => TestResponse::bytes(200, b"\x89PNG\r\n\x1a\n".to_vec()),
                    2 => TestResponse::bytes(200, b"not an image".to_vec()),
                    _ => TestResponse::bytes(200, vec![0; MAX_ARTWORK_BYTES as usize + 1]),
                };
                if case != 1 {
                    response.content_type = "image/png";
                }
                response
            });
            let client = RemoteClient::new(&server.url, "token").unwrap();
            assert!(
                client
                    .artwork(1, None, None, &AtomicBool::new(false))
                    .is_err(),
                "case {case}"
            );
        }
    }

    #[test]
    fn artwork_wait_is_cancelled_before_the_response_head() {
        let server = TestServer::start(|_| {
            TestResponse::bytes(200, Vec::new()).delayed(Duration::from_secs(3))
        });
        let client = RemoteClient::new(&server.url, "token").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let started = std::time::Instant::now();
        let pending = std::thread::spawn(move || client.artwork(1, None, None, &signal));
        std::thread::sleep(Duration::from_millis(150));
        cancel.store(true, Ordering::Release);
        assert!(matches!(
            pending.join().unwrap(),
            Err(RemoteError::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
