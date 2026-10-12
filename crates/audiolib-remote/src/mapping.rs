// crates/audiolib-remote/src/mapping.rs
//! Map server (macOS) absolute asset paths to WebDAV URLs through scan roots.
//!
//! Effective URL per root, in priority order:
//! 1. the user's per-root override (`webdav.root_urls[name]`, non-empty),
//! 2. the server's suggested `webdav_url` from `GET /roots`,
//! 3. the global quick option `webdav.base_url + "/" + <root name>`.
use crate::client::RemoteRoot;
use crate::error::{RemoteError, Result};
use crate::util::encode_path_segment;
use audiolib_core::config::WebDavSettings;
use serde::{Deserialize, Serialize};

/// Which setting produced a root's effective WebDAV URL.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UrlSource {
    User,
    Server,
    GlobalBase,
    Missing,
}

impl UrlSource {
    /// Short label for reports.
    pub fn label(self, chinese: bool) -> &'static str {
        match (self, chinese) {
            (Self::User, true) => "用户设置",
            (Self::User, false) => "user",
            (Self::Server, true) => "服务器默认",
            (Self::Server, false) => "server",
            (Self::GlobalBase, true) => "全局基址",
            (Self::GlobalBase, false) => "global base",
            (Self::Missing, true) => "未配置",
            (Self::Missing, false) => "not configured",
        }
    }
}

/// A server scan root with its resolved WebDAV URL.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectiveRoot {
    pub name: String,
    pub server_path: String,
    /// The server's suggested URL, kept for display as the default.
    pub server_url: Option<String>,
    /// Effective URL without trailing '/', or `None` when nothing is configured.
    pub url: Option<String>,
    pub source: UrlSource,
}

/// Resolve every root's effective URL from server data and the user's settings.
pub fn effective_roots(roots: &[RemoteRoot], webdav: &WebDavSettings) -> Vec<EffectiveRoot> {
    let base = webdav
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty());
    roots
        .iter()
        .map(|root| {
            let server_url = root
                .webdav_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_owned);
            let user = webdav
                .root_urls
                .get(&root.name)
                .map(|url| url.trim())
                .filter(|url| !url.is_empty());
            let (url, source) = if let Some(user) = user {
                (Some(user.to_owned()), UrlSource::User)
            } else if let Some(server) = &server_url {
                (Some(server.clone()), UrlSource::Server)
            } else if let Some(base) = base {
                (
                    Some(format!(
                        "{}/{}",
                        base.trim_end_matches('/'),
                        encode_path_segment(&root.name)
                    )),
                    UrlSource::GlobalBase,
                )
            } else {
                (None, UrlSource::Missing)
            };
            EffectiveRoot {
                name: root.name.clone(),
                server_path: root.server_path.clone(),
                server_url,
                url: url.map(|url| url.trim_end_matches('/').to_owned()),
                source,
            }
        })
        .collect()
}

fn normalized(path: &str) -> String {
    let path = path.replace('\\', "/");
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() && path.starts_with('/') {
        "/".into()
    } else {
        trimmed.to_owned()
    }
}

/// Relative remainder when `path` lies inside `root` on a component boundary.
fn relative<'a>(path: &'a str, root: &str, ignore_case: bool) -> Option<&'a str> {
    if root.is_empty() || path.len() < root.len() || !path.is_char_boundary(root.len()) {
        return None;
    }
    let (head, rest) = path.split_at(root.len());
    let same = if ignore_case {
        head.eq_ignore_ascii_case(root)
    } else {
        head == root
    };
    if !same {
        return None;
    }
    if root == "/" {
        return Some(rest);
    }
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix('/')
    }
}

/// The root containing `path` (already normalized) and the remainder: longest root wins;
/// exact-case matches first, then an ASCII case-insensitive match.
fn longest_root<'r, 'p, T>(
    path: &'p str,
    roots: &'r [T],
    server_path: impl Fn(&T) -> &str,
) -> Option<(&'r T, &'p str)> {
    [false, true].iter().find_map(|ignore_case| {
        roots
            .iter()
            .filter_map(|root| {
                let prefix = normalized(server_path(root));
                relative(path, &prefix, *ignore_case).map(|rest| (prefix.len(), root, rest))
            })
            .max_by_key(|(length, _, _)| *length)
            .map(|(_, root, rest)| (root, rest))
    })
}

/// Availability of a server scan root (`GET /roots` §6.4): a NAS share goes offline or
/// spins up as a whole, never file by file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootState {
    /// First check after server start not finished yet (treated as online).
    Checking,
    Online,
    /// The latest check has been slow for over 5 s (disk spinning up); slow is not offline.
    Waking,
    /// Definitely unreachable, or no answer for `root_offline_secs` (default 120 s).
    Offline,
}

impl RemoteRoot {
    /// The root's §6.4 status: `status` when the server sends a known value, otherwise
    /// `online: false` means offline; absent fields (older servers) mean online.
    pub fn state(&self) -> RootState {
        match self.status.as_deref().map(str::trim) {
            Some(status) if status.eq_ignore_ascii_case("offline") => RootState::Offline,
            Some(status) if status.eq_ignore_ascii_case("waking") => RootState::Waking,
            Some(status) if status.eq_ignore_ascii_case("checking") => RootState::Checking,
            Some(status) if status.eq_ignore_ascii_case("online") => RootState::Online,
            _ if self.online == Some(false) => RootState::Offline,
            _ => RootState::Online,
        }
    }
}

/// Short status of a root for lists and reports, e.g. `离线：Root directory not found`.
pub fn root_status_text(root: &RemoteRoot, chinese: bool) -> String {
    let reason = root
        .status_message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty());
    match (root.state(), chinese) {
        (RootState::Online, true) => "在线".into(),
        (RootState::Online, false) => "online".into(),
        (RootState::Checking, true) => "检查中".into(),
        (RootState::Checking, false) => "checking".into(),
        (RootState::Waking, true) => "磁盘唤醒中".into(),
        (RootState::Waking, false) => "waking (disk spinning up)".into(),
        (RootState::Offline, true) => match reason {
            Some(reason) => format!("离线：{reason}"),
            None => "离线".into(),
        },
        (RootState::Offline, false) => match reason {
            Some(reason) => format!("offline: {reason}"),
            None => "offline".into(),
        },
    }
}

/// The scan root that contains a server absolute path, if any.
pub fn root_for_path<'r>(path: &str, roots: &'r [RemoteRoot]) -> Option<&'r RemoteRoot> {
    let path = normalized(path);
    longest_root(&path, roots, |root| &root.server_path).map(|(root, _)| root)
}

/// Map a server absolute path to a WebDAV URL using the longest matching root.
/// Exact-case matches win; an ASCII case-insensitive match is the fallback because
/// macOS volumes are usually case-insensitive. Each segment is percent-encoded.
pub fn map_server_path(path: &str, roots: &[EffectiveRoot]) -> Result<String> {
    let path = normalized(path);
    let found = longest_root(&path, roots, |root| &root.server_path);
    let Some((root, rest)) = found else {
        return Err(RemoteError::NotFound(format!(
            "No server scan root contains {path}"
        )));
    };
    let Some(base) = &root.url else {
        return Err(RemoteError::Validation(format!(
            "No WebDAV URL is configured for root {}",
            root.name
        )));
    };
    let encoded = rest
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/");
    Ok(if encoded.is_empty() {
        base.clone()
    } else {
        format!("{base}/{encoded}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_for_path_picks_the_longest_containing_root() {
        let roots = vec![
            RemoteRoot {
                name: "ExampleLibrary".into(),
                server_path: "/volume1/ExampleLibrary".into(),
                ..Default::default()
            },
            RemoteRoot {
                name: "SFX".into(),
                server_path: "/volume1/ExampleLibrary/SFX/".into(),
                ..Default::default()
            },
        ];
        let found = |path| root_for_path(path, &roots).map(|root| root.name.as_str());
        assert_eq!(found("/volume1/ExampleLibrary/SFX/爆炸/a.wav"), Some("SFX"));
        assert_eq!(
            found("/volume1/examplelibrary/other/b.wav"),
            Some("ExampleLibrary")
        );
        assert_eq!(found("/volume1/ExampleLibrary_2/c.wav"), None);
        // §6.4 status; absent fields (older servers) mean online.
        let status = |online: Option<bool>, status: Option<&str>| {
            RemoteRoot {
                online,
                status: status.map(str::to_owned),
                ..Default::default()
            }
            .state()
        };
        assert_eq!(roots[1].state(), RootState::Online);
        assert_eq!(status(Some(true), Some("waking")), RootState::Waking);
        assert_eq!(status(Some(true), Some("checking")), RootState::Checking);
        assert_eq!(status(Some(false), Some("offline")), RootState::Offline);
        assert_eq!(status(Some(false), None), RootState::Offline);
        assert_eq!(status(Some(false), Some("future")), RootState::Offline);
        assert_eq!(status(None, Some("future")), RootState::Online);
    }

    fn roots() -> Vec<RemoteRoot> {
        vec![
            RemoteRoot {
                name: "SFX".into(),
                server_path: "/Volumes/Sound/SFX/".into(),
                webdav_url: Some("http://nas/dav/SFX/".into()),
                ..Default::default()
            },
            RemoteRoot {
                name: "SFX Aircraft".into(),
                server_path: "/Volumes/Sound/SFX/Aircraft".into(),
                webdav_url: None,
                ..Default::default()
            },
            RemoteRoot {
                name: "Foley".into(),
                server_path: "/Volumes/Sound/Foley".into(),
                webdav_url: None,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn user_override_beats_server_default_and_base_fills_gaps() {
        let mut settings = WebDavSettings {
            base_url: Some("http://mac.local:8080/webdav/".into()),
            ..Default::default()
        };
        settings
            .root_urls
            .insert("Foley".into(), "http://user/dav/Foley".into());
        settings.root_urls.insert("SFX".into(), "  ".into());
        let effective = effective_roots(&roots(), &settings);
        assert_eq!(effective[0].source, UrlSource::Server);
        assert_eq!(effective[0].url.as_deref(), Some("http://nas/dav/SFX"));
        assert_eq!(effective[1].source, UrlSource::GlobalBase);
        assert_eq!(
            effective[1].url.as_deref(),
            Some("http://mac.local:8080/webdav/SFX%20Aircraft")
        );
        assert_eq!(effective[2].source, UrlSource::User);
        settings.base_url = None;
        let effective = effective_roots(&roots(), &settings);
        assert_eq!(effective[1].source, UrlSource::Missing);
        assert!(map_server_path("/Volumes/Sound/SFX/Aircraft/a.wav", &effective).is_err());
    }

    #[test]
    fn longest_root_wins_and_segments_are_encoded() {
        let settings = WebDavSettings {
            base_url: Some("http://base".into()),
            ..Default::default()
        };
        let effective = effective_roots(&roots(), &settings);
        assert_eq!(
            map_server_path("/Volumes/Sound/SFX/Doors/门 #1 100%.wav", &effective).unwrap(),
            "http://nas/dav/SFX/Doors/%E9%97%A8%20%231%20100%25.wav"
        );
        assert_eq!(
            map_server_path("/Volumes/Sound/SFX/Aircraft/P-51/flyby.wav", &effective).unwrap(),
            "http://base/SFX%20Aircraft/P-51/flyby.wav"
        );
        assert_eq!(
            map_server_path("/volumes/sound/foley/Steps.wav", &effective).unwrap(),
            "http://base/Foley/Steps.wav"
        );
        assert!(map_server_path("/Volumes/Sound/SFXtra/a.wav", &effective).is_err());
        assert!(map_server_path("/Other/a.wav", &effective).is_err());
    }
}
