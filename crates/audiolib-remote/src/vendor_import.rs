// crates/audiolib-remote/src/vendor_import.rs
//! Vendor table import on the server (design §4.5, §4.7): preview (read-only), commit
//! with `Idempotency-Key`, rollback. Every call carries manage auth and
//! `X-AudioLib-Library-Id`; writes are sent once and never retried here.

use crate::client::RemoteClient;
use crate::error::{RemoteError, Result};
use audiolib_core::vendor_import::{VendorImportPreview, VendorImportResult};
use std::path::Path;
use std::time::Duration;

/// Path under the API base (`/api/v1`); design name `/api/vendor-import`.
pub const VENDOR_IMPORT_PATH: &str = "/vendor-import";
const MAX_REPLY_BYTES: u64 = 16 * 1024 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// `multipart/form-data` body with one `file` part. Quotes and line breaks in the name
/// are replaced so the header cannot be broken out of.
pub(crate) fn multipart_body(boundary: &str, file_name: &str, bytes: &[u8]) -> Vec<u8> {
    let safe: String = file_name
        .chars()
        .map(|c| {
            if matches!(c, '"' | '\r' | '\n' | '\\') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{safe}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

impl RemoteClient {
    fn vendor_call(
        &self,
        path: &str,
        content_type: Option<String>,
        body: Option<&[u8]>,
        idempotency_key: Option<&str>,
    ) -> Result<String> {
        let url = format!("{}{VENDOR_IMPORT_PATH}{path}", self.api);
        let context = format!("POST {VENDOR_IMPORT_PATH}{path}");
        let library_id = self.write_library_id()?;
        let compatibility = crate::compatibility::client_header()?;
        let agent = crate::client::agent(UPLOAD_TIMEOUT, Duration::from_secs(30));
        let mut request = agent
            .post(&url)
            .header(crate::client::LIBRARY_ID_HEADER, library_id.as_str())
            .header(
                audiolib_core::compatibility::CLIENT_COMPATIBILITY_HEADER,
                compatibility.as_str(),
            );
        if !self.token.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.token));
        }
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        let result = match (content_type, body) {
            (Some(kind), Some(body)) => request.header("Content-Type", kind).send(body),
            _ => request.send_empty(),
        };
        let response = result.map_err(|error| RemoteError::from_transport(error, &url))?;
        let mut response = self.checked(response, &context)?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_REPLY_BYTES)
            .read_to_string()
            .map_err(|error| RemoteError::Protocol(format!("{context}: {error}")))
    }

    /// `POST /vendor-import/preview` with the table as a multipart upload. Read-only on
    /// the server; returns the `import_id` used by commit and rollback.
    pub fn vendor_import_preview(&self, file: &Path) -> Result<VendorImportPreview> {
        let bytes = std::fs::read(file)
            .map_err(|error| RemoteError::Validation(format!("{}: {error}", file.display())))?;
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "vendor.csv".into());
        let boundary = format!(
            "audiolib-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let body = multipart_body(&boundary, &name, &bytes);
        let text = self.vendor_call(
            "/preview",
            Some(format!("multipart/form-data; boundary={boundary}")),
            Some(&body),
            None,
        )?;
        let preview: VendorImportPreview = serde_json::from_str(&text)
            .map_err(|error| RemoteError::Protocol(format!("vendor preview: {error}")))?;
        if preview.import_id.trim().is_empty() {
            return Err(RemoteError::Protocol("vendor preview: no import_id".into()));
        }
        Ok(preview)
    }

    /// `POST /vendor-import/{id}/commit`; replaying the same key returns the same result.
    pub fn vendor_import_commit(&self, import_id: &str, key: &str) -> Result<VendorImportResult> {
        let id = crate::util::encode_path_segment(import_id);
        let text = self.vendor_call(&format!("/{id}/commit"), None, None, Some(key))?;
        serde_json::from_str(&text)
            .map_err(|error| RemoteError::Protocol(format!("vendor commit: {error}")))
    }

    /// `POST /vendor-import/{id}/rollback`.
    pub fn vendor_import_rollback(&self, import_id: &str) -> Result<VendorImportResult> {
        let id = crate::util::encode_path_segment(import_id);
        let text = self.vendor_call(&format!("/{id}/rollback"), None, None, None)?;
        serde_json::from_str(&text)
            .map_err(|error| RemoteError::Protocol(format!("vendor rollback: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_body_wraps_one_file_part_and_sanitizes_name() {
        let body = multipart_body("b", "a\"b\r\n.csv", b"x,y");
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with("--b\r\n"));
        assert!(text.contains("filename=\"a_b__.csv\""));
        assert!(text.ends_with("\r\nx,y\r\n--b--\r\n"));
    }
}
