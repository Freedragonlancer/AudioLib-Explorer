// crates/audiolib-remote/src/profiles.rs
//! Server profile operations that change settings and secrets together: the current
//! single-server dialog saved against the active profile, "use the external address",
//! switching the active source and removing a profile. Pure functions on owned values;
//! the caller saves (`AppConfig::save`, [`crate::save_secrets`]) and swaps providers.
use crate::secrets::RemoteSecrets;
use audiolib_core::config::{
    ActiveSource, DataSourceMode, DataSourceSettings, ProfileKind, ServerProfile,
};
use audiolib_core::profiles::{
    MAX_PROFILE_NAME_CHARS, default_profile_name, default_transcode, same_server_url,
};
use audiolib_core::server_url::{Scheme, ServerAddress};
use audiolib_core::{Error, Result};

/// What the single-server data source dialog submits besides the settings themselves.
#[derive(Clone, Copy, Default)]
pub struct ConnectionForm<'a> {
    /// Token field (trimmed; empty = no token).
    pub token: &'a str,
    /// Global WebDAV password field.
    pub webdav_password: &'a str,
    /// The server's external address (`/health` `public_url`) the user chose with "use
    /// the external address" in this dialog session. When the submitted URL is that
    /// address, a sibling external profile is created or updated instead of
    /// overwriting the current (LAN) profile.
    pub external_hint: Option<&'a str>,
    /// Name for a newly created external profile (localized by the caller).
    pub external_name: Option<&'a str>,
    /// "Transcode auditions" checkbox, `Some` only when the user changed it in this dialog
    /// session; `None` gives the resulting profile [`untouched_transcode`].
    pub transcode: Option<bool>,
}

impl std::fmt::Debug for ConnectionForm<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectionForm")
            .field("token", &"<redacted>")
            .field("external_hint", &self.external_hint)
            .field("transcode", &self.transcode)
            .finish_non_exhaustive()
    }
}

/// The "transcode auditions" value a dialog save gives the profile it lands on when the
/// user did not touch the checkbox, for the address `url` typed into the dialog (the
/// dialog shows this value):
/// * a URL already saved in a profile: that profile's choice;
/// * the external address offered by the connection test: the existing external sibling's
///   choice, else on (a new external profile);
/// * the shown profile edited in place: its choice, unless the address moved between LAN
///   and external, then the new address's [`default_transcode`];
/// * otherwise (a new profile, an invalid address): the address default.
pub fn untouched_transcode(
    previous: &DataSourceSettings,
    url: &str,
    external_hint: Option<&str>,
) -> bool {
    let Ok(address) = ServerAddress::parse(url.trim()) else {
        return previous.audition_transcode();
    };
    let url = address.base_url();
    if let Some(profile) = previous.profile_for_url(&url) {
        return profile.transcode_enabled();
    }
    let shown = previous
        .legacy_profile_id()
        .and_then(|id| previous.profile(&id).cloned());
    if let (Some(hint), Some(from)) = (external_hint, &shown)
        && same_server_url(hint, &url)
    {
        return previous
            .profiles
            .iter()
            .find(|profile| {
                profile.id != from.id
                    && profile.kind == ProfileKind::External
                    && profile.group_key() == from.group_key()
            })
            .is_none_or(|sibling| sibling.transcode_enabled());
    }
    match shown {
        Some(profile) if default_transcode(&profile.server_url) == default_transcode(&url) => {
            profile.transcode_enabled()
        }
        _ => default_transcode(&url),
    }
}

/// Apply the existing single-server dialog to the profile model.
///
/// `form` is `previous` with the dialog's fields written over it (`mode`, `server_url`,
/// WebDAV, sync settings). The dialog shows the legacy profile (active, or most recently
/// used in local mode), so:
/// * a URL already saved in another profile selects that profile;
/// * the external address offered by the connection test becomes a sibling profile;
/// * any other URL edits the shown profile in place (same id, token and group);
/// * with no profile yet, a valid URL creates one.
///
/// The token field is saved to the resulting profile, except that an untouched token
/// does not overwrite the token another existing profile already has. Server mode
/// activates the profile; local mode activates the local library. The old fields and the
/// top-level token are mirrored for older clients.
pub fn apply_connection_form(
    previous: &DataSourceSettings,
    previous_secrets: &RemoteSecrets,
    form: DataSourceSettings,
    input: &ConnectionForm<'_>,
) -> Result<(DataSourceSettings, RemoteSecrets)> {
    let transcode = input
        .transcode
        .unwrap_or_else(|| untouched_transcode(previous, &form.server_url, input.external_hint));
    let mut settings = form;
    settings.active = previous.active.clone();
    settings.profiles = previous.profiles.clone();
    let mut secrets = previous_secrets.clone();
    secrets.reconcile(previous);
    secrets.webdav_password = input.webdav_password.to_owned();
    let token = input.token.trim();
    let shown_token = previous_secrets.token.trim().to_owned();
    let shown = previous.legacy_profile_id();
    let url = settings.server_url.trim().to_owned();
    let address = ServerAddress::parse(&url).ok();
    let target = match address {
        None => None,
        Some(address) => {
            let url = address.base_url();
            let shown_profile = shown
                .as_deref()
                .and_then(|id| settings.profile(id))
                .cloned();
            if let Some(profile) = settings.profile_for_url(&url) {
                Some(profile.id.clone())
            } else if let (Some(hint), Some(from)) = (input.external_hint, &shown_profile)
                && same_server_url(hint, &url)
            {
                Some(settings.upsert_external_profile(&from.id, &url, input.external_name)?)
            } else if let Some(from) = shown_profile {
                let profile = settings
                    .profile_mut(&from.id)
                    .expect("shown profile exists");
                if profile.name == default_profile_name(&profile.server_url) {
                    profile.name = default_profile_name(&url);
                }
                profile.kind = ProfileKind::guess(&url);
                profile.server_url = url;
                Some(from.id)
            } else {
                Some(settings.add_profile("", &url)?)
            }
        }
    };
    if let Some(profile) = target.as_deref().and_then(|id| settings.profile_mut(id)) {
        profile.transcode = Some(transcode);
    }
    match &target {
        Some(id) => {
            let keep_own = token == shown_token
                && shown.as_deref() != Some(id.as_str())
                && !secrets.token_for(id).is_empty();
            if !keep_own {
                secrets.set_token(id, token);
            }
        }
        // No profile to hold it (local mode with an empty or invalid URL): keep the
        // single top-level token exactly as earlier releases did.
        None => secrets.token = token.to_owned(),
    }
    match (settings.mode, target) {
        (DataSourceMode::Server, Some(id)) => settings.set_active(&ActiveSource::Profile(id))?,
        (DataSourceMode::Server, None) => {
            return Err(Error::Validation(
                "Server mode needs a valid server address".into(),
            ));
        }
        (DataSourceMode::Local, target) => {
            if settings.active.is_some() || !settings.profiles.is_empty() {
                settings.set_active(&ActiveSource::Local)?;
            }
            // The edited profile is now the most recently used server.
            if let Some(profile) = target.as_deref().and_then(|id| settings.profile(id)) {
                settings.server_url = profile.server_url.clone();
            }
        }
    }
    secrets.mirror(&settings);
    Ok((settings, secrets))
}

/// "Use the external address": create or update the external sibling of profile `from`
/// ([`DataSourceSettings::upsert_external_profile`]) and give it `from`'s token when it
/// has none. Returns the sibling's id; the active source is unchanged.
pub fn use_external_address(
    settings: &mut DataSourceSettings,
    secrets: &mut RemoteSecrets,
    from: &str,
    public_url: &str,
    name: Option<&str>,
) -> Result<String> {
    let id = settings.upsert_external_profile(from, public_url, name)?;
    if secrets.token_for(&id).is_empty() {
        let token = secrets.token_for(from).to_owned();
        secrets.set_token(&id, &token);
    }
    secrets.mirror(settings);
    Ok(id)
}

/// Make `source` active in settings and secrets (mirrors for older clients). The caller
/// saves both and swaps the provider through the library operation path.
pub fn activate_source(
    settings: &mut DataSourceSettings,
    secrets: &mut RemoteSecrets,
    source: &ActiveSource,
) -> Result<()> {
    settings.set_active(source)?;
    secrets.mirror(settings);
    Ok(())
}

/// The connection page of one profile in the unified "data source and library" window.
#[derive(Clone, Copy)]
pub struct ProfileEdit<'a> {
    /// Display name (trimmed; empty = the address's host and port).
    pub name: &'a str,
    /// Address as typed (normalised to its base URL).
    pub server_url: &'a str,
    /// `None` keeps the saved token (the window never shows it); `Some("")` clears it.
    pub token: Option<&'a str>,
    /// The "transcode auditions" checkbox.
    pub transcode: bool,
}

impl std::fmt::Debug for ProfileEdit<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProfileEdit")
            .field("name", &self.name)
            .field("server_url", &self.server_url)
            .field("token", &self.token.map(|_| "<redacted>"))
            .field("transcode", &self.transcode)
            .finish()
    }
}

/// Save the connection page of profile `id` (`None`: a new profile) in place.
///
/// Unlike [`apply_connection_form`] (the old single-server dialog, which edited the active
/// profile and matched URLs against the others), the window edits exactly the selected
/// profile: an address already saved in another profile is refused, and the active source
/// does not change (switching is a separate action, [`activate_source`]). A changed
/// address forgets the profile's library identity and last test (they belonged to the old
/// address) and re-guesses its kind. An active profile is re-mirrored into the old fields
/// for older clients. Returns the profile id.
pub fn save_profile_edit(
    settings: &mut DataSourceSettings,
    secrets: &mut RemoteSecrets,
    id: Option<&str>,
    edit: &ProfileEdit<'_>,
) -> Result<String> {
    // As typed in the window: no scheme means HTTPS.
    let address = ServerAddress::parse_input(edit.server_url, Scheme::Https)
        .map_err(|error| Error::Validation(error.to_string()))?;
    let url = address.base_url();
    if let Some(other) = settings.profiles.iter().find(|profile| {
        Some(profile.id.as_str()) != id && same_server_url(&profile.server_url, &url)
    }) {
        return Err(Error::Validation(format!(
            "The address is already saved in the server profile \"{}\"",
            other.name
        )));
    }
    let name = edit.name.trim();
    if name.chars().count() > MAX_PROFILE_NAME_CHARS {
        return Err(Error::Validation(format!(
            "A profile name has at most {MAX_PROFILE_NAME_CHARS} characters"
        )));
    }
    secrets.reconcile(settings);
    let id = match id {
        Some(id) => {
            let profile = settings
                .profile_mut(id)
                .ok_or_else(|| Error::NotFound(format!("Server profile {id}")))?;
            if !same_server_url(&profile.server_url, &url) {
                profile.kind = ProfileKind::guess(&url);
                profile.library_id = None;
                profile.last_test = None;
            }
            profile.name = if name.is_empty() {
                default_profile_name(&url)
            } else {
                name.to_owned()
            };
            profile.server_url = url;
            id.to_owned()
        }
        None => settings.add_profile(name, &url)?,
    };
    if let Some(profile) = settings.profile_mut(&id) {
        profile.transcode = Some(edit.transcode);
    }
    if let Some(token) = edit.token {
        secrets.set_token(&id, token);
    }
    if settings.active.as_deref() == Some(id.as_str()) {
        settings.mirror_active();
    }
    secrets.mirror(settings);
    Ok(id)
}

/// The "transcode auditions" box of the window's connection page while the user has not
/// clicked it: the saved profile's choice, unless the address being typed moves between a
/// LAN and an external address, then the new address's [`default_transcode`] (also for a
/// new profile).
pub fn edit_transcode(saved: Option<&ServerProfile>, url: &str) -> bool {
    match saved {
        Some(profile) if default_transcode(&profile.server_url) == default_transcode(url) => {
            profile.transcode_enabled()
        }
        _ => default_transcode(url),
    }
}

/// Remove a profile and its secrets (an active profile falls back to local mode).
pub fn remove_server_profile(
    settings: &mut DataSourceSettings,
    secrets: &mut RemoteSecrets,
    id: &str,
) -> Result<ServerProfile> {
    let removed = settings.remove_profile(id)?;
    secrets.remove_profile(id);
    secrets.mirror(settings);
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::config::{DEFAULT_PROFILE_ID, ProfileMigration};

    #[test]
    fn the_window_edits_exactly_the_selected_profile_without_switching() {
        let mut settings = DataSourceSettings {
            mode: DataSourceMode::Server,
            server_url: LAN.into(),
            ..DataSourceSettings::default()
        };
        settings.migrate_profiles();
        let mut secrets = RemoteSecrets {
            token: "lan".into(),
            ..RemoteSecrets::default()
        };
        secrets.reconcile(&settings);
        // A new profile: not activated, its own token, transcode as ticked.
        let edit = ProfileEdit {
            name: " 家里 Mac（外网） ",
            server_url: "audiolib.example.com:8443",
            token: Some(" ext "),
            transcode: true,
        };
        assert!(!format!("{edit:?}").contains("ext "));
        let id = save_profile_edit(&mut settings, &mut secrets, None, &edit).unwrap();
        let profile = settings.profile(&id).unwrap();
        assert_eq!(profile.name, "家里 Mac（外网）");
        assert_eq!(profile.server_url, EXT);
        assert_eq!(profile.kind, ProfileKind::External);
        assert_eq!(profile.transcode, Some(true));
        assert_eq!(settings.active.as_deref(), Some(DEFAULT_PROFILE_ID));
        assert_eq!(settings.server_url, LAN);
        assert_eq!(secrets.token_for(&id), "ext");
        assert_eq!(
            secrets.token, "lan",
            "the top-level token stays the active one's"
        );
        consistent(&settings);
        // An address saved in another profile is refused; nothing changes.
        let before = (settings.clone(), secrets.clone());
        let duplicate = ProfileEdit {
            server_url: "http://server.example.lan:8790/",
            ..edit
        };
        assert!(save_profile_edit(&mut settings, &mut secrets, Some(&id), &duplicate).is_err());
        assert!(save_profile_edit(&mut settings, &mut secrets, None, &duplicate).is_err());
        assert_eq!((settings.clone(), secrets.clone()), before);
        // Rename the active profile, keep its token (None), clear nothing: mirrored.
        settings
            .record_test(DEFAULT_PROFILE_ID, true, "ok", 1, Some("lib-1"))
            .unwrap();
        let rename = ProfileEdit {
            name: "Home LAN",
            server_url: LAN,
            token: None,
            transcode: false,
        };
        save_profile_edit(
            &mut settings,
            &mut secrets,
            Some(DEFAULT_PROFILE_ID),
            &rename,
        )
        .unwrap();
        let lan = settings.profile(DEFAULT_PROFILE_ID).unwrap();
        assert_eq!(lan.name, "Home LAN");
        assert_eq!(lan.library_id.as_deref(), Some("lib-1"));
        assert!(lan.last_test.is_some(), "same address keeps the last test");
        assert_eq!(secrets.token_for(DEFAULT_PROFILE_ID), "lan");
        assert!(settings.audition_original, "mirrored for older builds");
        // Moving the active profile to another address forgets its identity, mirrors the
        // URL, and an empty name follows the address; Some("") clears the token.
        let moved = ProfileEdit {
            name: "",
            server_url: "http://10.0.0.5:8790",
            token: Some(""),
            transcode: false,
        };
        save_profile_edit(
            &mut settings,
            &mut secrets,
            Some(DEFAULT_PROFILE_ID),
            &moved,
        )
        .unwrap();
        let lan = settings.profile(DEFAULT_PROFILE_ID).unwrap();
        assert_eq!(lan.name, "10.0.0.5:8790");
        assert!(lan.library_id.is_none() && lan.last_test.is_none());
        assert_eq!(settings.server_url, "http://10.0.0.5:8790");
        assert_eq!(secrets.token_for(DEFAULT_PROFILE_ID), "");
        assert_eq!(secrets.token, "");
        assert_eq!(secrets.token_for(&id), "ext");
        consistent(&settings);
        // Invalid input and unknown ids are refused.
        let bad = ProfileEdit {
            server_url: "ftp://x",
            ..moved
        };
        assert!(save_profile_edit(&mut settings, &mut secrets, None, &bad).is_err());
        let long = "x".repeat(65);
        let named = ProfileEdit {
            name: &long,
            server_url: "http://10.0.0.6:8790",
            ..moved
        };
        assert!(save_profile_edit(&mut settings, &mut secrets, None, &named).is_err());
        assert!(save_profile_edit(&mut settings, &mut secrets, Some("nope"), &moved).is_err());
    }

    #[test]
    fn the_untouched_transcode_box_follows_the_address_class() {
        let lan_on = ServerProfile {
            id: "a".into(),
            server_url: LAN.into(),
            transcode: Some(true),
            ..ServerProfile::default()
        };
        assert!(edit_transcode(Some(&lan_on), "http://10.0.0.9:8790"));
        assert!(edit_transcode(Some(&lan_on), EXT));
        let lan_off = ServerProfile {
            transcode: Some(false),
            ..lan_on.clone()
        };
        assert!(!edit_transcode(Some(&lan_off), "http://10.0.0.9:8790"));
        assert!(edit_transcode(Some(&lan_off), EXT));
        assert!(!edit_transcode(None, LAN));
        assert!(edit_transcode(None, EXT));
        assert!(!edit_transcode(None, ""));
    }

    const LAN: &str = "http://server.example.lan:8790";
    const EXT: &str = "https://audiolib.example.com:8443";

    fn form(previous: &DataSourceSettings, mode: DataSourceMode, url: &str) -> DataSourceSettings {
        DataSourceSettings {
            mode,
            server_url: url.into(),
            ..previous.clone()
        }
    }

    fn input(token: &str) -> ConnectionForm<'_> {
        ConnectionForm {
            token,
            ..ConnectionForm::default()
        }
    }

    fn consistent(settings: &DataSourceSettings) {
        let mut copy = settings.clone();
        assert_eq!(copy.migrate_profiles(), ProfileMigration::Unchanged);
        settings.validate().unwrap();
    }

    #[test]
    fn first_server_from_the_dialog_becomes_the_active_profile() {
        let previous = DataSourceSettings::default();
        let (settings, secrets) = apply_connection_form(
            &previous,
            &RemoteSecrets::default(),
            form(&previous, DataSourceMode::Server, LAN),
            &input(" tok "),
        )
        .unwrap();
        let profile = settings.active_profile().unwrap();
        assert_eq!(profile.server_url, LAN);
        assert_eq!(secrets.token_for(&profile.id), "tok");
        assert_eq!(secrets.token, "tok");
        assert_eq!(settings.mode, DataSourceMode::Server);
        consistent(&settings);
        // Local mode with nothing valid typed stores the token the old way.
        let (local, secrets) = apply_connection_form(
            &previous,
            &RemoteSecrets::default(),
            form(&previous, DataSourceMode::Local, "half typed"),
            &input("t"),
        )
        .unwrap();
        assert!(local.profiles.is_empty() && local.active.is_none());
        assert_eq!(local.server_url, "half typed");
        assert_eq!(secrets.token, "t");
        assert!(
            apply_connection_form(
                &previous,
                &RemoteSecrets::default(),
                form(&previous, DataSourceMode::Server, "nope"),
                &input(""),
            )
            .is_err()
        );
    }

    #[test]
    fn dialog_edits_the_active_profile_and_selects_saved_siblings() {
        let mut previous = DataSourceSettings {
            mode: DataSourceMode::Server,
            server_url: LAN.into(),
            ..DataSourceSettings::default()
        };
        previous.migrate_profiles();
        let mut secrets = RemoteSecrets {
            token: "lan".into(),
            ..RemoteSecrets::default()
        };
        secrets.reconcile(&previous);
        // The LAN IP changed: same profile, edited in place.
        let (moved, moved_secrets) = apply_connection_form(
            &previous,
            &secrets,
            form(
                &previous,
                DataSourceMode::Server,
                "http://server2.example.lan:8790",
            ),
            &input("lan"),
        )
        .unwrap();
        assert_eq!(moved.profiles.len(), 1);
        let profile = moved.active_profile().unwrap();
        assert_eq!(profile.id, DEFAULT_PROFILE_ID);
        assert_eq!(profile.server_url, "http://server2.example.lan:8790");
        assert_eq!(profile.name, "server2.example.lan:8790");
        assert_eq!(moved_secrets.token_for(DEFAULT_PROFILE_ID), "lan");
        consistent(&moved);
        // "Use external address" then save: a sibling, the LAN profile untouched.
        let (external, external_secrets) = apply_connection_form(
            &previous,
            &secrets,
            form(&previous, DataSourceMode::Server, EXT),
            &ConnectionForm {
                token: "lan",
                external_hint: Some(EXT),
                external_name: Some("家里（外网）"),
                ..ConnectionForm::default()
            },
        )
        .unwrap();
        assert_eq!(external.profiles.len(), 2);
        assert_eq!(
            external.profile(DEFAULT_PROFILE_ID).unwrap().server_url,
            LAN
        );
        let sibling = external.active_profile().unwrap().clone();
        assert_eq!(sibling.server_url, EXT);
        assert_eq!(sibling.kind, ProfileKind::External);
        assert_eq!(sibling.name, "家里（外网）");
        assert_eq!(
            external_secrets.token_for(&sibling.id),
            "lan",
            "token copied"
        );
        assert_eq!(external_secrets.token_for(DEFAULT_PROFILE_ID), "lan");
        consistent(&external);
        // Give the sibling its own token, then switch back by typing the LAN URL: the
        // untouched token field does not overwrite the LAN token (or the sibling's).
        let mut secrets = external_secrets.clone();
        secrets.set_token(&sibling.id, "ext");
        secrets.mirror(&external);
        assert_eq!(secrets.token, "ext");
        let (back, back_secrets) = apply_connection_form(
            &external,
            &secrets,
            form(&external, DataSourceMode::Server, &format!("{LAN}/")),
            &input("ext"),
        )
        .unwrap();
        assert_eq!(back.active.as_deref(), Some(DEFAULT_PROFILE_ID));
        assert_eq!(back_secrets.token, "lan");
        assert_eq!(back_secrets.token_for(&sibling.id), "ext");
        consistent(&back);
        // A token typed in the dialog is saved to the profile being selected.
        let (_, typed) = apply_connection_form(
            &external,
            &secrets,
            form(&external, DataSourceMode::Server, LAN),
            &input("fresh"),
        )
        .unwrap();
        assert_eq!(typed.token_for(DEFAULT_PROFILE_ID), "fresh");
        assert_eq!(typed.token_for(&sibling.id), "ext");
        // Local mode keeps the profiles, activates local and edits the shown token.
        let (local, local_secrets) = apply_connection_form(
            &external,
            &secrets,
            form(&external, DataSourceMode::Local, EXT),
            &input(""),
        )
        .unwrap();
        assert_eq!(local.active_source(), ActiveSource::Local);
        assert_eq!(local.mode, DataSourceMode::Local);
        assert_eq!(local.profiles.len(), 2);
        assert_eq!(local.server_url, EXT);
        assert_eq!(local_secrets.token_for(&sibling.id), "");
        assert_eq!(local_secrets.token_for(DEFAULT_PROFILE_ID), "lan");
        consistent(&local);
    }

    #[test]
    fn the_dialog_saves_the_transcode_choice_per_profile() {
        // First LAN server, checkbox untouched: off (and mirrored for older builds).
        let previous = DataSourceSettings::default();
        assert!(!untouched_transcode(&previous, LAN, None));
        let (lan, secrets) = apply_connection_form(
            &previous,
            &RemoteSecrets::default(),
            form(&previous, DataSourceMode::Server, LAN),
            &input(""),
        )
        .unwrap();
        let lan_id = lan.active_profile().unwrap().id.clone();
        assert_eq!(lan.active_profile().unwrap().transcode, Some(false));
        assert!(!lan.audition_transcode() && lan.audition_original);
        consistent(&lan);
        // The user ticks the box: on for this profile only.
        let ticked = ConnectionForm {
            transcode: Some(true),
            ..input("")
        };
        let (on, secrets) = apply_connection_form(
            &lan,
            &secrets,
            form(&lan, DataSourceMode::Server, LAN),
            &ticked,
        )
        .unwrap();
        assert!(on.audition_transcode() && !on.audition_original);
        consistent(&on);
        // Untouched again: the stored choice stays (the dialog shows it too).
        assert!(untouched_transcode(&on, LAN, None));
        let (kept, secrets) = apply_connection_form(
            &on,
            &secrets,
            form(&on, DataSourceMode::Server, LAN),
            &input(""),
        )
        .unwrap();
        assert!(kept.audition_transcode());
        // "Use the external address" in this session: a new external sibling, on; the LAN
        // profile keeps its own value.
        let (off_lan, secrets) = apply_connection_form(
            &kept,
            &secrets,
            form(&kept, DataSourceMode::Server, LAN),
            &ConnectionForm {
                transcode: Some(false),
                ..input("")
            },
        )
        .unwrap();
        assert!(untouched_transcode(&off_lan, EXT, Some(EXT)));
        let external = ConnectionForm {
            external_hint: Some(EXT),
            ..input("tok")
        };
        let (with_external, secrets) = apply_connection_form(
            &off_lan,
            &secrets,
            form(&off_lan, DataSourceMode::Server, EXT),
            &external,
        )
        .unwrap();
        let active = with_external.active_profile().unwrap();
        assert_eq!(active.kind, ProfileKind::External);
        assert_eq!(active.transcode, Some(true));
        assert_eq!(
            with_external.profile(&lan_id).unwrap().transcode,
            Some(false)
        );
        consistent(&with_external);
        // Selecting the saved LAN URL again: its own choice (off), not the shown profile's.
        assert!(!untouched_transcode(&with_external, LAN, None));
        let (back, _) = apply_connection_form(
            &with_external,
            &secrets,
            form(&with_external, DataSourceMode::Server, LAN),
            &input(""),
        )
        .unwrap();
        assert!(!back.audition_transcode() && back.audition_original);
        // Editing the shown LAN profile to another LAN address keeps its choice; moving it
        // to a public address takes the external default.
        let mut lan_on = back.clone();
        lan_on.profile_mut(&lan_id).unwrap().transcode = Some(true);
        assert!(untouched_transcode(&lan_on, "http://10.0.0.7:8790", None));
        assert!(!untouched_transcode(&back, "http://10.0.0.7:8790", None));
        assert!(untouched_transcode(
            &back,
            "https://other.example.org",
            None
        ));
        // Local mode saves the edited profile's choice without activating it.
        let (local, _) = apply_connection_form(
            &back,
            &RemoteSecrets::default(),
            form(&back, DataSourceMode::Local, LAN),
            &ticked,
        )
        .unwrap();
        assert_eq!(local.profile(&lan_id).unwrap().transcode, Some(true));
        assert_eq!(local.active_source(), ActiveSource::Local);
        consistent(&local);
    }

    #[test]
    fn switching_and_removing_keep_tokens_mirrored() {
        let mut settings = DataSourceSettings {
            mode: DataSourceMode::Server,
            server_url: LAN.into(),
            ..DataSourceSettings::default()
        };
        settings.migrate_profiles();
        let mut secrets = RemoteSecrets {
            token: "lan".into(),
            ..RemoteSecrets::default()
        };
        secrets.reconcile(&settings);
        let ext = use_external_address(&mut settings, &mut secrets, DEFAULT_PROFILE_ID, EXT, None)
            .unwrap();
        assert_eq!(secrets.token_for(&ext), "lan");
        assert_eq!(settings.active.as_deref(), Some(DEFAULT_PROFILE_ID));
        secrets.set_token(&ext, "ext");
        activate_source(
            &mut settings,
            &mut secrets,
            &ActiveSource::Profile(ext.clone()),
        )
        .unwrap();
        assert_eq!(
            (settings.server_url.as_str(), secrets.token.as_str()),
            (EXT, "ext")
        );
        // A second "use external address" keeps the sibling's own token.
        use_external_address(&mut settings, &mut secrets, DEFAULT_PROFILE_ID, EXT, None).unwrap();
        assert_eq!(secrets.token_for(&ext), "ext");
        remove_server_profile(&mut settings, &mut secrets, &ext).unwrap();
        assert_eq!(settings.active_source(), ActiveSource::Local);
        assert!(!secrets.profiles.contains_key(&ext));
        assert!(activate_source(&mut settings, &mut secrets, &ActiveSource::Profile(ext)).is_err());
        activate_source(
            &mut settings,
            &mut secrets,
            &ActiveSource::Profile(DEFAULT_PROFILE_ID.into()),
        )
        .unwrap();
        assert_eq!(
            (settings.server_url.as_str(), secrets.token.as_str()),
            (LAN, "lan")
        );
        let debug = format!("{:?}", input("secret-value"));
        assert!(!debug.contains("secret-value"));
    }
}
