//! ZOUGCLOUD(ZC-004/005/006): Tauri commands for the optional Steam integration.
//!
//! Thin glue only. The real work lives in the `steam` crate (Steam discovery,
//! shortcuts.vdf and artwork), in `process::resolve` (turning a Drop launch
//! option into a concrete executable) and in `crate::steamgriddb` (artwork
//! sources). Keeping this file small keeps the amount of ZougCloud code sitting
//! inside upstream's `src/` to a minimum.

use std::{fmt::Display, path::PathBuf, sync::Arc, time::Duration};

use log::{info, warn};
use process::{
    error::ProcessError,
    resolve::{ResolvedLaunch, resolve_launch_target, resolve_launch_targets},
};
use serde::{Deserialize, Serialize};
use serde_with::SerializeDisplay;
use steam::{
    ArtworkKind, ShortcutRecord, ShortcutRequest, SteamError, SteamInstall, SteamUser,
    find_shortcut, installed_artwork, launch_steam, locate_steam, remove_artwork, remove_shortcut,
    library_url, request_steam_exit, upsert_shortcut, wait_for_steam_exit, write_artwork,
};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::steamgriddb;

/// How long to let Steam wind down before giving up. Steam refuses to exit
/// while a game is running, and that is the case we want to report rather than
/// wait out.
const STEAM_EXIT_TIMEOUT: Duration = Duration::from_secs(25);

// Debug is hand-written below: upstream's ProcessError implements Display but
// not Debug, so it cannot be derived here.
#[derive(SerializeDisplay, Clone)]
pub enum SteamCommandError {
    Steam(SteamError),
    Process(ProcessError),
    /// The chosen launch option resolves to a file that is not on disk. A
    /// shortcut pointing at nothing would fail silently inside Steam.
    NoExecutable(String),
    Artwork(String),
    Opener(Arc<tauri_plugin_opener::Error>),
    Io(String),
}

impl Display for SteamCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SteamCommandError::Steam(e) => write!(f, "{e}"),
            SteamCommandError::Process(e) => write!(f, "{e}"),
            SteamCommandError::NoExecutable(p) => {
                write!(f, "The game executable could not be found: {p}")
            }
            SteamCommandError::Artwork(e) => write!(f, "Could not fetch artwork: {e}"),
            SteamCommandError::Opener(e) => write!(f, "Could not open Steam: {e}"),
            SteamCommandError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::fmt::Debug for SteamCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl From<SteamError> for SteamCommandError {
    fn from(value: SteamError) -> Self {
        SteamCommandError::Steam(value)
    }
}

impl From<ProcessError> for SteamCommandError {
    fn from(value: ProcessError) -> Self {
        SteamCommandError::Process(value)
    }
}

/// Drop's own images for a game, passed down from the frontend, which already
/// has them. Used when SteamGridDB has nothing or no key is configured, so a
/// game never ends up as a blank tile.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DropArtwork {
    pub cover_object_id: Option<String>,
    pub banner_object_id: Option<String>,
    pub icon_object_id: Option<String>,
}

/// Everything the game's Steam panel needs in one round trip.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SteamGameStatus {
    /// None when Steam is not installed, so the UI can hide the feature
    /// entirely rather than offering something that cannot work.
    pub install: Option<SteamInstall>,
    /// The existing shortcut for this game, if any, for the account checked.
    pub shortcut: Option<ShortcutRecord>,
    /// Launch options that can become a Steam shortcut. Emulator launches are
    /// excluded: they run the emulator with the game as an argument, so there
    /// is no single executable to point Steam at.
    pub launches: Vec<ResolvedLaunch>,
    /// Artwork slots already on disk for the existing shortcut.
    pub artwork: Vec<ArtworkKind>,
    /// Whether a SteamGridDB key is stored. The key itself never leaves the
    /// backend.
    pub steamgriddb_configured: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AddShortcutOutcome {
    pub shortcut: ShortcutRecord,
    pub artwork: Vec<ArtworkKind>,
    /// True when we shut Steam down and started it again, so the UI can say so.
    pub steam_restarted: bool,
}

fn user_for(install: &SteamInstall, account_id: Option<u32>) -> Result<&SteamUser, SteamError> {
    match account_id {
        Some(id) => install
            .users
            .iter()
            .find(|u| u.account_id == id)
            .ok_or(SteamError::UnknownUser(id)),
        // Users are sorted most-recently-used first.
        None => install.users.first().ok_or(SteamError::NoUsers),
    }
}

/// Close Steam if it is running, so our writes survive.
///
/// Steam holds shortcuts.vdf in memory and rewrites it on exit, and it only
/// scans the artwork directory at startup — so editing either while it runs is
/// pointless at best. Rather than refusing and leaving the member to work out
/// what to do, we shut Steam down cleanly, make the change, and start it again:
/// they get the shortcut *and* its artwork visible straight away.
///
/// Never a kill. `-shutdown` is Steam's own graceful exit and it declines while
/// a game is running, which is exactly the case we want surfaced instead of
/// forced.
fn close_steam_if_running(install: &SteamInstall) -> Result<bool, SteamCommandError> {
    if !install.running {
        return Ok(false);
    }

    info!("closing Steam so shortcut and artwork changes survive");
    request_steam_exit(install)?;

    if !wait_for_steam_exit(STEAM_EXIT_TIMEOUT) {
        return Err(SteamError::SteamWillNotClose.into());
    }

    Ok(true)
}

fn restart_steam(install: &SteamInstall, was_running: bool) {
    if !was_running {
        return;
    }
    // Best effort: the change is already on disk, so failing to restart Steam
    // is an inconvenience, not a failure of what the member asked for.
    if let Err(e) = launch_steam(install) {
        warn!("could not restart Steam: {e}");
    }
}

/// Fill in as many artwork slots as we can.
///
/// SteamGridDB first when a key is configured, then Drop's own images. Every
/// slot is independent and every failure is swallowed: artwork is a nicety, and
/// losing it must never fail the thing the member actually asked for, which is
/// getting the game into Steam.
async fn apply_artwork(
    user: &SteamUser,
    app_id: u32,
    game_name: &str,
    drop_artwork: &DropArtwork,
) -> Vec<ArtworkKind> {
    let key = steamgriddb::load_key();
    let sgdb_game = match &key {
        Some(key) => steamgriddb::find_game(key, game_name).await,
        None => None,
    };

    let mut written = Vec::new();

    for kind in ArtworkKind::ALL {
        let mut bytes: Option<Vec<u8>> = None;

        if let (Some(key), Some(game)) = (&key, sgdb_game) {
            bytes = steamgriddb::fetch_asset(key, game, kind).await;
        }

        if bytes.is_none()
            && let Some(object_id) = drop_fallback(kind, drop_artwork)
        {
            match steamgriddb::fetch_drop_object(object_id).await {
                Ok(data) => bytes = Some(data),
                Err(e) => warn!("no Drop fallback artwork for {kind:?}: {e}"),
            }
        }

        let Some(bytes) = bytes else { continue };

        match write_artwork(user, app_id, kind, &bytes) {
            Ok(_) => written.push(kind),
            Err(e) => warn!("could not write {kind:?} artwork: {e}"),
        }
    }

    written
}

/// Which Drop image stands in for each Steam slot.
///
/// There is no Drop equivalent of Steam's transparent logo treatment, so that
/// slot is left empty rather than filled with a cover that would sit wrongly
/// over the hero.
fn drop_fallback(kind: ArtworkKind, artwork: &DropArtwork) -> Option<&str> {
    match kind {
        ArtworkKind::Portrait => artwork.cover_object_id.as_deref(),
        ArtworkKind::Capsule | ArtworkKind::Hero => artwork.banner_object_id.as_deref(),
        ArtworkKind::Icon => artwork.icon_object_id.as_deref(),
        ArtworkKind::Logo => None,
    }
}

#[tauri::command]
pub fn steam_game_status(
    game_id: String,
    app_name: String,
    account_id: Option<u32>,
) -> Result<SteamGameStatus, SteamCommandError> {
    let launches = resolve_launch_targets(&game_id).unwrap_or_default();
    let steamgriddb_configured = steamgriddb::is_configured();

    let install = match locate_steam() {
        Ok(install) => install,
        Err(SteamError::NotInstalled | SteamError::NoUsers) => {
            return Ok(SteamGameStatus {
                install: None,
                shortcut: None,
                launches,
                artwork: Vec::new(),
                steamgriddb_configured,
            });
        }
        Err(e) => return Err(e.into()),
    };

    // A shortcut is looked up against whichever launch option currently
    // resolves; matching also falls back to the game name, so a shortcut added
    // for a different launch option is still found.
    let user = user_for(&install, account_id)?;
    let shortcut = launches
        .iter()
        .find_map(|launch| find_shortcut(user, &app_name, &launch.exe).ok().flatten());

    let artwork = shortcut
        .as_ref()
        .map(|s| installed_artwork(user, s.app_id))
        .unwrap_or_default();

    Ok(SteamGameStatus {
        install: Some(install.clone()),
        shortcut,
        launches,
        artwork,
        steamgriddb_configured,
    })
}

#[tauri::command]
pub async fn steam_add_shortcut(
    game_id: String,
    app_name: String,
    launch_index: usize,
    account_id: Option<u32>,
    drop_artwork: Option<DropArtwork>,
) -> Result<AddShortcutOutcome, SteamCommandError> {
    let install = locate_steam()?;
    let launch = resolve_launch_target(&game_id, launch_index)?;

    if !launch.exists {
        return Err(SteamCommandError::NoExecutable(
            launch.exe.display().to_string(),
        ));
    }

    let steam_restarted = close_steam_if_running(&install)?;

    let request = ShortcutRequest {
        app_name: app_name.clone(),
        exe: launch.exe.clone(),
        start_dir: launch.working_dir.clone(),
        launch_options: launch.args.join(" "),
        icon: None,
    };

    let record = {
        let user = user_for(&install, account_id)?;
        upsert_shortcut(user, &request)
    };

    // Whatever happens next, Steam must come back up if we took it down.
    let record = match record {
        Ok(record) => record,
        Err(e) => {
            restart_steam(&install, steam_restarted);
            return Err(e.into());
        }
    };

    let artwork = {
        let user = user_for(&install, account_id)?;
        apply_artwork(
            user,
            record.app_id,
            &app_name,
            &drop_artwork.unwrap_or_default(),
        )
        .await
    };

    restart_steam(&install, steam_restarted);

    info!(
        "added game {game_id} to Steam as app {} with {} artwork slot(s)",
        record.app_id,
        artwork.len()
    );

    Ok(AddShortcutOutcome {
        shortcut: record,
        artwork,
        steam_restarted,
    })
}

#[tauri::command]
pub fn steam_remove_shortcut(
    app_id: u32,
    account_id: Option<u32>,
) -> Result<bool, SteamCommandError> {
    let install = locate_steam()?;
    let steam_restarted = close_steam_if_running(&install)?;

    let result = {
        let user = user_for(&install, account_id)?;
        remove_shortcut(user, app_id).map(|()| {
            // The app id is what tied these files to the entry; with the entry
            // gone they can only ever be dead weight in Steam's grid folder.
            let removed = remove_artwork(user, app_id);
            info!("removed Steam shortcut {app_id} and {removed} artwork file(s)");
        })
    };

    restart_steam(&install, steam_restarted);
    result?;
    Ok(steam_restarted)
}

/// Open the game's page in Steam.
///
/// Read-only by construction: it follows a URL and never opens shortcuts.vdf,
/// so the shortcut's AppID, artwork, Steam playtime and controller settings are
/// all untouched. Safe while Steam is running.
#[tauri::command]
pub fn steam_open_shortcut(app_id: u32, app_handle: AppHandle) -> Result<(), SteamCommandError> {
    let url = library_url(app_id);
    app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| SteamCommandError::Opener(Arc::new(e)))
}

/// Where this account's custom artwork lives, so the UI can offer to open it.
#[tauri::command]
pub fn steam_grid_dir(account_id: Option<u32>) -> Result<PathBuf, SteamCommandError> {
    let install = locate_steam()?;
    Ok(user_for(&install, account_id)?.grid_dir())
}

/// Store or clear the SteamGridDB key.
///
/// Deliberately write-only: the key is never handed back to the frontend, only
/// the fact that one exists. Pass an empty string to forget it.
#[tauri::command]
pub fn steam_set_steamgriddb_key(key: Option<String>) -> Result<bool, SteamCommandError> {
    steamgriddb::save_key(key.as_deref())
        .map_err(|e| SteamCommandError::Io(format!("Could not save the SteamGridDB key: {e}")))?;
    Ok(steamgriddb::is_configured())
}
