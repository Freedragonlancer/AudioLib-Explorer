// crates/audiolib-db/tests/smv2/http.rs
//! Plain HTTP/1.0 client for the pending server tests (no client dependency).
//!
//! Env: AUDIOLIB_SMV2_ADMIN_URL (http://host:port of a disposable test instance),
//! AUDIOLIB_SMV2_ADMIN_DB (the library file that instance serves; the tests drive
//! the migration on it with `audiolib-migrate` and mutate it), optional
//! AUDIOLIB_SMV2_ADMIN_TOKEN (Bearer).
//!
//! Library endpoints are not in this repo; defaults below are placeholders, each
//! overridable with AUDIOLIB_SMV2_HTTP_<NAME> (e.g. AUDIOLIB_SMV2_HTTP_PLAYBACK).
//! `{id}` is replaced with a library row id.
//!
//! Search `scope` (server 2026-11-03): `main|all|path`; omit = `main` (= schema 8).
//! MAIN_SEARCH omits scope on purpose. Vendor import: `/api/vendor-import` (+ `/*`).
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    time::Duration,
};

pub const PENDING: &str =
    "pending server migration HTTP (set AUDIOLIB_SMV2_ADMIN_URL, AUDIOLIB_SMV2_ADMIN_DB)";

pub const ADMIN: &str = "/admin/api/migration";

/// (name, method, default path, body). Writes are the vendor import and annotations.
pub const ENDPOINTS: &[(&str, &str, &str, Option<&str>)] = &[
    ("LIST", "GET", "/api/library/files?limit=5", None),
    (
        // omit scope (= main / s8)
        "MAIN_SEARCH",
        "GET",
        "/api/search?q=zildjian",
        None,
    ),
    ("PATH_SEARCH", "GET", "/api/search?q=Urban&scope=path", None),
    (
        "ALL_SEARCH",
        "GET",
        "/api/search?q=zildjian&scope=all",
        None,
    ),
    ("PLAYBACK", "GET", "/api/files/{id}/stream", None),
    ("ARTWORK", "GET", "/api/files/{id}/artwork", None),
    ("VENDOR_IMPORT", "POST", "/api/vendor-import", Some("{}")),
    (
        "ANNOTATIONS",
        "POST",
        "/api/files/{id}/annotations",
        Some(r#"{"tags":"smv2"}"#),
    ),
];

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
    pub fn json(&self) -> Value {
        serde_json::from_str(self.body.trim()).unwrap_or(Value::Null)
    }
}

pub struct Server {
    host: String,
    token: Option<String>,
    pub db: PathBuf,
}

impl Server {
    pub fn from_env() -> Server {
        let url = std::env::var("AUDIOLIB_SMV2_ADMIN_URL").expect("AUDIOLIB_SMV2_ADMIN_URL");
        let host = url
            .strip_prefix("http://")
            .expect("AUDIOLIB_SMV2_ADMIN_URL must be http://host:port")
            .trim_end_matches('/')
            .to_string();
        Server {
            host,
            token: std::env::var("AUDIOLIB_SMV2_ADMIN_TOKEN").ok(),
            db: std::env::var("AUDIOLIB_SMV2_ADMIN_DB")
                .expect("AUDIOLIB_SMV2_ADMIN_DB")
                .into(),
        }
    }

    pub fn request(&self, method: &str, path: &str, body: Option<&str>) -> Response {
        let mut s = TcpStream::connect(&self.host).expect("connect AUDIOLIB_SMV2_ADMIN_URL");
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let auth = self
            .token
            .as_deref()
            .map(|t| format!("Authorization: Bearer {t}\r\n"))
            .unwrap_or_default();
        let body = body.unwrap_or("");
        write!(
            s,
            "{method} {path} HTTP/1.0\r\nHost: {}\r\nAccept: application/json\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            self.host,
            body.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP response");
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|l| l.split(' ').nth(1))
            .and_then(|c| c.parse().ok())
            .expect("status line");
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        Response {
            status,
            headers,
            body: String::from_utf8_lossy(&raw[split + 4..]).into_owned(),
        }
    }

    pub fn status(&self) -> Response {
        self.request("GET", &format!("{ADMIN}/status"), None)
    }

    pub fn admin(&self, action: &str) -> Response {
        self.request("POST", &format!("{ADMIN}/{action}"), Some("{}"))
    }

    /// Call a library endpoint by name (see [`ENDPOINTS`]).
    pub fn call(&self, name: &str, id: i64) -> Response {
        let (_, method, default, body) = ENDPOINTS
            .iter()
            .find(|e| e.0 == name)
            .unwrap_or_else(|| panic!("unknown endpoint {name}"));
        let path = std::env::var(format!("AUDIOLIB_SMV2_HTTP_{name}"))
            .unwrap_or_else(|_| default.to_string())
            .replace("{id}", &id.to_string());
        self.request(method, &path, *body)
    }
}
