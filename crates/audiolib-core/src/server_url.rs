// crates/audiolib-core/src/server_url.rs
//! Server address entry and normalisation for server mode.
//!
//! The user enters either a full URL (`https://audiolib.example.com:18086/audiolib`) or a
//! scheme + host + port (+ optional path prefix). Both normalise to one base URL, which is
//! what `data_source.server_url` stores; the API base is that URL plus `/api/v1`. A pasted
//! `/api/v1` suffix is accepted and not doubled. Saved values from earlier versions (such
//! as `http://192.168.1.10:8790`) parse to the same base URL and the same API base, so no
//! settings migration is needed.
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// Path appended to the base URL to reach the v1 API.
pub const API_SUFFIX: &str = "/api/v1";
/// Longest accepted server URL.
pub const MAX_SERVER_URL_LEN: usize = 2048;

/// Transport scheme. New entries default to HTTPS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scheme {
    #[default]
    Https,
    Http,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Http => "http",
        }
    }

    /// Port used when the address does not name one.
    pub fn default_port(self) -> u16 {
        match self {
            Self::Https => 443,
            Self::Http => 80,
        }
    }
}

/// A validated server address. `host` keeps the user's spelling (IPv6 in brackets);
/// `prefix` is empty or `/segment[/segment…]` without a trailing slash and without the
/// `/api/v1` suffix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    pub scheme: Scheme,
    pub host: String,
    pub port: Option<u16>,
    pub prefix: String,
}

/// Why a server address was rejected. `Display` is English; [`ServerUrlError::message`]
/// gives the desktop's Chinese or English text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerUrlError {
    Empty,
    MissingScheme,
    UnsupportedScheme(String),
    MissingHost,
    Credentials,
    Whitespace,
    QueryOrFragment,
    TooLong,
    InvalidHost(String),
    InvalidIpv4(String),
    NonAsciiHost(String),
    InvalidPort(String),
    InvalidPath(String),
}

impl ServerUrlError {
    /// User-facing text in the desktop language.
    pub fn message(&self, chinese: bool) -> String {
        if !chinese {
            return self.to_string();
        }
        match self {
            Self::Empty => "请填写服务器地址，例如 https://audiolib.example.com:18086 或 http://192.168.1.10:8790".into(),
            Self::MissingScheme => "服务器地址需以 http:// 或 https:// 开头".into(),
            Self::UnsupportedScheme(scheme) => {
                format!("不支持的协议“{scheme}”：只能使用 http 或 https")
            }
            Self::MissingHost => "请填写服务器域名或 IP 地址".into(),
            Self::Credentials => {
                "服务器地址不能包含用户名或密码（访问令牌请填写在“访问令牌”中）".into()
            }
            Self::Whitespace => "服务器地址不能包含空格".into(),
            Self::QueryOrFragment => "服务器地址不能包含 ? 或 #".into(),
            Self::TooLong => format!("服务器地址过长（最多 {MAX_SERVER_URL_LEN} 个字符）"),
            Self::InvalidHost(host) => format!(
                "主机名“{host}”无效：只能包含字母、数字、连字符和点（每段 1–63 个字符），例如 audiolib.example.com；IPv6 地址需加方括号"
            ),
            Self::InvalidIpv4(host) => {
                format!("IPv4 地址“{host}”无效：应为 4 段 0–255 的数字，例如 192.168.1.10")
            }
            Self::NonAsciiHost(host) => {
                format!("主机名“{host}”包含非 ASCII 字符：请使用 Punycode 形式（xn--…）")
            }
            Self::InvalidPort(port) => format!("端口“{port}”无效：应为 1–65535 的整数"),
            Self::InvalidPath(path) => format!(
                "路径前缀“{path}”无效：只能包含字母、数字和 - . _ ~ 等 URL 字符，不能包含 . 或 .. 段、反斜杠或非 ASCII 字符"
            ),
        }
    }
}

impl fmt::Display for ServerUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str(
                "Enter the server address, e.g. https://audiolib.example.com:18086 or http://192.168.1.10:8790",
            ),
            Self::MissingScheme => {
                formatter.write_str("The server URL must start with http:// or https://")
            }
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "Unsupported scheme \"{scheme}\": use http or https")
            }
            Self::MissingHost => formatter.write_str("Enter the server domain name or IP address"),
            Self::Credentials => formatter.write_str(
                "The server address must not contain a user name or password (put the token in Access token)",
            ),
            Self::Whitespace => formatter.write_str("The server address must not contain spaces"),
            Self::QueryOrFragment => {
                formatter.write_str("The server address must not contain ? or #")
            }
            Self::TooLong => write!(
                formatter,
                "The server address is too long (at most {MAX_SERVER_URL_LEN} characters)"
            ),
            Self::InvalidHost(host) => write!(
                formatter,
                "Invalid host name \"{host}\": use letters, digits, hyphens and dots (1-63 characters per label), e.g. audiolib.example.com; put IPv6 addresses in brackets"
            ),
            Self::InvalidIpv4(host) => write!(
                formatter,
                "Invalid IPv4 address \"{host}\": use four numbers 0-255, e.g. 192.168.1.10"
            ),
            Self::NonAsciiHost(host) => write!(
                formatter,
                "Host name \"{host}\" contains non-ASCII characters: use its punycode form (xn--...)"
            ),
            Self::InvalidPort(port) => {
                write!(formatter, "Invalid port \"{port}\": use a whole number 1-65535")
            }
            Self::InvalidPath(path) => write!(
                formatter,
                "Invalid path prefix \"{path}\": use URL characters (letters, digits, - . _ ~ ...), no . or .. segments, backslashes or non-ASCII characters"
            ),
        }
    }
}

impl std::error::Error for ServerUrlError {}

impl From<ServerUrlError> for crate::Error {
    fn from(error: ServerUrlError) -> Self {
        crate::Error::Validation(error.to_string())
    }
}

impl ServerAddress {
    /// Parse a saved or typed full URL; the scheme is required. Used for stored
    /// `server_url` values and by the HTTP client.
    pub fn parse(input: &str) -> Result<Self, ServerUrlError> {
        Self::parse_with_default(input, None)
    }

    /// Parse user input from the URL field: like [`ServerAddress::parse`], but an input
    /// without `scheme://` (e.g. `audiolib.example.com:18086`) uses `default` (HTTPS in
    /// the dialog).
    pub fn parse_input(input: &str, default: Scheme) -> Result<Self, ServerUrlError> {
        Self::parse_with_default(input, Some(default))
    }

    fn parse_with_default(input: &str, default: Option<Scheme>) -> Result<Self, ServerUrlError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ServerUrlError::Empty);
        }
        if trimmed.len() > MAX_SERVER_URL_LEN {
            return Err(ServerUrlError::TooLong);
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(ServerUrlError::Whitespace);
        }
        if trimmed.contains(['?', '#']) {
            return Err(ServerUrlError::QueryOrFragment);
        }
        let (scheme, rest) = match trimmed.find("://") {
            Some(index) => {
                let name = &trimmed[..index];
                let scheme = if name.eq_ignore_ascii_case("https") {
                    Scheme::Https
                } else if name.eq_ignore_ascii_case("http") {
                    Scheme::Http
                } else {
                    return Err(ServerUrlError::UnsupportedScheme(name.to_owned()));
                };
                (scheme, &trimmed[index + 3..])
            }
            None => (default.ok_or(ServerUrlError::MissingScheme)?, trimmed),
        };
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(ServerUrlError::Credentials);
        }
        let (host, port) = split_authority(authority)?;
        Ok(Self {
            scheme,
            host,
            port,
            prefix: normalize_prefix(path)?,
        })
    }

    /// Build an address from the dialog's separate fields. Each field is trimmed; an empty
    /// port uses the scheme's default port and an empty prefix means none.
    pub fn from_parts(
        scheme: Scheme,
        host: &str,
        port: &str,
        prefix: &str,
    ) -> Result<Self, ServerUrlError> {
        let host = host.trim();
        if host.is_empty() {
            return Err(ServerUrlError::MissingHost);
        }
        if host.chars().any(char::is_whitespace) {
            return Err(ServerUrlError::Whitespace);
        }
        if host.contains('@') {
            return Err(ServerUrlError::Credentials);
        }
        if host.contains(['/', '?', '#']) || host.contains("://") {
            return Err(ServerUrlError::InvalidHost(host.to_owned()));
        }
        let host = if host.starts_with('[') {
            host.to_owned()
        } else if host.parse::<Ipv6Addr>().is_ok() {
            // A bare IPv6 literal in the host field is unambiguous: bracket it.
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        validate_host(&host)?;
        let port = port.trim();
        let port = if port.is_empty() {
            None
        } else {
            Some(parse_port(port)?)
        };
        let prefix = prefix.trim();
        if prefix.chars().any(char::is_whitespace) {
            return Err(ServerUrlError::Whitespace);
        }
        if prefix.contains(['?', '#']) {
            return Err(ServerUrlError::QueryOrFragment);
        }
        Ok(Self {
            scheme,
            host,
            port,
            prefix: normalize_prefix(prefix)?,
        })
    }

    /// `scheme://host[:port][/prefix]`: the value stored as `server_url`.
    pub fn base_url(&self) -> String {
        match self.port {
            Some(port) => format!(
                "{}://{}:{port}{}",
                self.scheme.as_str(),
                self.host,
                self.prefix
            ),
            None => format!("{}://{}{}", self.scheme.as_str(), self.host, self.prefix),
        }
    }

    /// `<base_url>/api/v1`.
    pub fn api_url(&self) -> String {
        format!("{}{API_SUFFIX}", self.base_url())
    }

    /// Port actually connected to (explicit or the scheme default).
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(self.scheme.default_port())
    }

    /// Host without IPv6 brackets, for DNS resolution and TLS server names.
    pub fn bare_host(&self) -> &str {
        self.host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(&self.host)
    }

    /// True for an IPv4 or IPv6 literal (a TLS certificate rarely covers those).
    pub fn host_is_ip(&self) -> bool {
        self.bare_host().parse::<std::net::IpAddr>().is_ok()
    }
}

/// Normalise dialog URL input to the stored base URL (scheme defaults to HTTPS).
pub fn normalize_server_url(input: &str) -> Result<String, ServerUrlError> {
    Ok(ServerAddress::parse_input(input, Scheme::Https)?.base_url())
}

/// API base (`…/api/v1`) of a stored server URL; the scheme is required.
pub fn api_base_url(server_url: &str) -> Result<String, ServerUrlError> {
    Ok(ServerAddress::parse(server_url)?.api_url())
}

fn split_authority(authority: &str) -> Result<(String, Option<u16>), ServerUrlError> {
    if authority.is_empty() {
        return Err(ServerUrlError::MissingHost);
    }
    let (host, port) = if authority.starts_with('[') {
        let end = authority
            .find(']')
            .ok_or_else(|| ServerUrlError::InvalidHost(authority.to_owned()))?;
        let (host, rest) = authority.split_at(end + 1);
        match rest {
            "" => (host, None),
            _ => match rest.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None => return Err(ServerUrlError::InvalidHost(authority.to_owned())),
            },
        }
    } else if authority.matches(':').count() > 1 {
        // An IPv6 literal without brackets cannot carry a port unambiguously.
        return Err(ServerUrlError::InvalidHost(authority.to_owned()));
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err(ServerUrlError::MissingHost);
    }
    validate_host(host)?;
    let port = port.map(parse_port).transpose()?;
    Ok((host.to_owned(), port))
}

fn parse_port(text: &str) -> Result<u16, ServerUrlError> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ServerUrlError::InvalidPort(text.to_owned()));
    }
    match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(ServerUrlError::InvalidPort(text.to_owned())),
    }
}

fn validate_host(host: &str) -> Result<(), ServerUrlError> {
    if host.is_empty() {
        return Err(ServerUrlError::MissingHost);
    }
    if let Some(inner) = host.strip_prefix('[') {
        return match inner.strip_suffix(']').map(str::parse::<Ipv6Addr>) {
            Some(Ok(_)) => Ok(()),
            _ => Err(ServerUrlError::InvalidHost(host.to_owned())),
        };
    }
    if !host.is_ascii() {
        return Err(ServerUrlError::NonAsciiHost(host.to_owned()));
    }
    if host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return host
            .parse::<Ipv4Addr>()
            .map(|_| ())
            .map_err(|_| ServerUrlError::InvalidIpv4(host.to_owned()));
    }
    // Underscores are tolerated for LAN (NetBIOS-style) names saved by earlier versions.
    let valid_label = |label: &str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    };
    if host.len() > 253 || !host.split('.').all(valid_label) {
        return Err(ServerUrlError::InvalidHost(host.to_owned()));
    }
    Ok(())
}

/// `""`, `"/"`, `"/audiolib/"`, `"/audiolib/api/v1"` → `""`, `""`, `"/audiolib"`,
/// `"/audiolib"`. Empty segments (`//`, trailing `/`) are dropped.
fn normalize_prefix(path: &str) -> Result<String, ServerUrlError> {
    if path.contains('\\') || !path.is_ascii() {
        return Err(ServerUrlError::InvalidPath(path.to_owned()));
    }
    let mut segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    for segment in &segments {
        if matches!(*segment, "." | "..") || !valid_segment(segment) {
            return Err(ServerUrlError::InvalidPath(path.to_owned()));
        }
    }
    let len = segments.len();
    if len >= 2
        && segments[len - 2].eq_ignore_ascii_case("api")
        && segments[len - 1].eq_ignore_ascii_case("v1")
    {
        segments.truncate(len - 2);
    }
    Ok(segments
        .iter()
        .map(|segment| format!("/{segment}"))
        .collect())
}

/// RFC 3986 `pchar`: unreserved, sub-delims, `:`, `@` and well-formed `%XX` escapes.
fn valid_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let hex = |offset: usize| bytes.get(index + offset).is_some_and(u8::is_ascii_hexdigit);
            if !(hex(1) && hex(2)) {
                return false;
            }
            index += 3;
            continue;
        }
        let allowed = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
            );
        if !allowed {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(input: &str) -> String {
        normalize_server_url(input).unwrap_or_else(|error| panic!("{input}: {error}"))
    }

    #[test]
    fn full_urls_normalise_to_a_base_url_and_api_base() {
        assert_eq!(
            base("https://audiolib.example.com:18086"),
            "https://audiolib.example.com:18086"
        );
        assert_eq!(
            api_base_url("https://audiolib.example.com:18086").unwrap(),
            "https://audiolib.example.com:18086/api/v1"
        );
        // Surrounding whitespace and a mixed-case scheme are tolerated.
        assert_eq!(
            base("  HTTPS://example.com:443/  "),
            "https://example.com:443"
        );
        assert_eq!(base("https://example.com"), "https://example.com");
    }

    #[test]
    fn path_prefix_trailing_slash_and_pasted_api_suffix() {
        assert_eq!(
            base("https://host:18086/audiolib"),
            "https://host:18086/audiolib"
        );
        assert_eq!(
            base("https://host:18086/audiolib/"),
            "https://host:18086/audiolib"
        );
        assert_eq!(
            base("https://host:18086/audiolib//"),
            "https://host:18086/audiolib"
        );
        // Pasted API URLs are not doubled.
        for pasted in [
            "https://host:18086/audiolib/api/v1",
            "https://host:18086/audiolib/api/v1/",
            "https://host:18086/audiolib/API/V1",
        ] {
            assert_eq!(base(pasted), "https://host:18086/audiolib", "{pasted}");
            assert_eq!(
                api_base_url(pasted).unwrap(),
                "https://host:18086/audiolib/api/v1"
            );
        }
        assert_eq!(base("http://h:8790/api/v1"), "http://h:8790");
        assert_eq!(
            base("https://h/a/b-c_d.e~f/%E9%9F%B3"),
            "https://h/a/b-c_d.e~f/%E9%9F%B3"
        );
        let address = ServerAddress::parse("https://h:1/x/y/").unwrap();
        assert_eq!(address.prefix, "/x/y");
    }

    #[test]
    fn legacy_saved_urls_keep_their_api_base() {
        // Exactly what earlier clients derived (so cache.json server identities match).
        for (saved, api) in [
            (
                "http://192.168.1.10:8790",
                "http://192.168.1.10:8790/api/v1",
            ),
            (
                "http://192.168.1.10:8790/",
                "http://192.168.1.10:8790/api/v1",
            ),
            ("http://h/api/v1/", "http://h/api/v1"),
            (
                "http://NAS.example.lan:8790",
                "http://NAS.example.lan:8790/api/v1",
            ),
            ("http://mac_nas:8790", "http://mac_nas:8790/api/v1"),
            ("https://host:8790/", "https://host:8790/api/v1"),
        ] {
            assert_eq!(api_base_url(saved).unwrap(), api, "{saved}");
        }
        let legacy = ServerAddress::parse("http://192.168.1.10:8790").unwrap();
        assert_eq!(legacy.scheme, Scheme::Http);
        assert_eq!(legacy.host, "192.168.1.10");
        assert_eq!(legacy.port, Some(8790));
        assert!(legacy.host_is_ip());
        assert_eq!(legacy.base_url(), "http://192.168.1.10:8790");
    }

    #[test]
    fn domain_and_port_fields_compose_a_base_url() {
        let address =
            ServerAddress::from_parts(Scheme::Https, " audiolib.example.com ", " 18086 ", "")
                .unwrap();
        assert_eq!(address.base_url(), "https://audiolib.example.com:18086");
        assert_eq!(address.effective_port(), 18086);
        let prefixed =
            ServerAddress::from_parts(Scheme::Https, "host", "18086", "audiolib/").unwrap();
        assert_eq!(prefixed.base_url(), "https://host:18086/audiolib");
        let pasted =
            ServerAddress::from_parts(Scheme::Https, "host", "18086", "/audiolib/api/v1").unwrap();
        assert_eq!(pasted.api_url(), "https://host:18086/audiolib/api/v1");
        // Empty port = scheme default; nothing is written into the URL.
        let default = ServerAddress::from_parts(Scheme::Https, "example.com", "", "").unwrap();
        assert_eq!(default.base_url(), "https://example.com");
        assert_eq!(default.effective_port(), 443);
        let lan = ServerAddress::from_parts(Scheme::Http, "192.168.1.10", "8790", "").unwrap();
        assert_eq!(lan.base_url(), "http://192.168.1.10:8790");
        let v6 = ServerAddress::from_parts(Scheme::Http, "::1", "8790", "").unwrap();
        assert_eq!(v6.base_url(), "http://[::1]:8790");
        assert_eq!(v6.bare_host(), "::1");
        // Round trip: a parsed URL fills the fields that compose it again.
        let parsed = ServerAddress::parse("https://host:18086/audiolib").unwrap();
        let port = parsed.port.map(|port| port.to_string()).unwrap_or_default();
        let again =
            ServerAddress::from_parts(parsed.scheme, &parsed.host, &port, &parsed.prefix).unwrap();
        assert_eq!(again, parsed);
    }

    #[test]
    fn bare_input_defaults_to_https_but_stored_urls_need_a_scheme() {
        assert_eq!(
            base("audiolib.example.com:18086"),
            "https://audiolib.example.com:18086"
        );
        assert_eq!(
            ServerAddress::parse_input("10.0.0.2:8790", Scheme::Http)
                .unwrap()
                .base_url(),
            "http://10.0.0.2:8790"
        );
        assert_eq!(
            ServerAddress::parse("h:8790"),
            Err(ServerUrlError::MissingScheme)
        );
    }

    #[test]
    fn ipv4_and_ipv6_hosts() {
        assert_eq!(base("http://10.0.0.1:1"), "http://10.0.0.1:1");
        assert_eq!(
            base("https://[2001:db8::1]:18086"),
            "https://[2001:db8::1]:18086"
        );
        assert!(matches!(
            normalize_server_url("http://192.168.1.300:8790"),
            Err(ServerUrlError::InvalidIpv4(_))
        ));
        assert!(matches!(
            normalize_server_url("http://1.2.3:8790"),
            Err(ServerUrlError::InvalidIpv4(_))
        ));
        assert!(matches!(
            normalize_server_url("http://2001:db8::1:8790"),
            Err(ServerUrlError::InvalidHost(_))
        ));
        assert!(matches!(
            normalize_server_url("http://[zz::1]:8790"),
            Err(ServerUrlError::InvalidHost(_))
        ));
    }

    #[test]
    fn invalid_input_has_specific_errors_in_both_languages() {
        let cases: &[(&str, ServerUrlError)] = &[
            ("", ServerUrlError::Empty),
            ("   ", ServerUrlError::Empty),
            (
                "ftp://host",
                ServerUrlError::UnsupportedScheme("ftp".into()),
            ),
            ("https://", ServerUrlError::MissingHost),
            ("https://:8790", ServerUrlError::MissingHost),
            ("http://user:pw@host:8790", ServerUrlError::Credentials),
            ("http://ho st:8790", ServerUrlError::Whitespace),
            ("http://host:8790/?x=1", ServerUrlError::QueryOrFragment),
            ("http://host:8790#top", ServerUrlError::QueryOrFragment),
            ("http://host:0", ServerUrlError::InvalidPort("0".into())),
            (
                "http://host:65536",
                ServerUrlError::InvalidPort("65536".into()),
            ),
            ("http://host:80a", ServerUrlError::InvalidPort("80a".into())),
            ("http://host:", ServerUrlError::InvalidPort(String::new())),
            (
                "https://-bad.example.com",
                ServerUrlError::InvalidHost("-bad.example.com".into()),
            ),
            ("https://a..b", ServerUrlError::InvalidHost("a..b".into())),
            (
                "https://exa!mple.com",
                ServerUrlError::InvalidHost("exa!mple.com".into()),
            ),
            (
                "https://音效.com",
                ServerUrlError::NonAsciiHost("音效.com".into()),
            ),
            (
                "https://h/a/../b",
                ServerUrlError::InvalidPath("/a/../b".into()),
            ),
            (
                "https://h/a\\b",
                ServerUrlError::InvalidPath("/a\\b".into()),
            ),
            (
                "https://h/50%zz",
                ServerUrlError::InvalidPath("/50%zz".into()),
            ),
        ];
        for (input, expected) in cases {
            let error = normalize_server_url(input).unwrap_err();
            assert_eq!(&error, expected, "{input}");
            assert!(!error.message(true).is_empty() && !error.message(false).is_empty());
            assert_ne!(error.message(true), error.message(false), "{input}");
        }
        assert!(
            ServerUrlError::InvalidPort("99999".into())
                .message(true)
                .contains("1–65535")
        );
        assert!(
            ServerUrlError::InvalidPort("99999".into())
                .message(false)
                .contains("1-65535")
        );
        let long = format!("https://h/{}", "a".repeat(MAX_SERVER_URL_LEN));
        assert_eq!(normalize_server_url(&long), Err(ServerUrlError::TooLong));
        // Field-level errors.
        assert_eq!(
            ServerAddress::from_parts(Scheme::Https, " ", "1", ""),
            Err(ServerUrlError::MissingHost)
        );
        assert!(matches!(
            ServerAddress::from_parts(Scheme::Https, "https://host", "1", ""),
            Err(ServerUrlError::InvalidHost(_))
        ));
        assert!(matches!(
            ServerAddress::from_parts(Scheme::Https, "host", "abc", ""),
            Err(ServerUrlError::InvalidPort(_))
        ));
        assert!(matches!(
            ServerAddress::from_parts(Scheme::Https, "host", "1", "a b"),
            Err(ServerUrlError::Whitespace)
        ));
        // Converted to the core validation error for settings validation.
        assert!(matches!(
            crate::Error::from(ServerUrlError::Empty),
            crate::Error::Validation(_)
        ));
    }
}
