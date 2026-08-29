//! ZOUGCLOUD(ZC-004/005/006): Tauri commands for the optional Steam integration.
//!
//! Thin glue only. The real work lives in the `steam` crate (Steam discovery and
//! shortcuts.vdf) and in `process::resolve` (turning a Drop launch option into a
//! concrete executable). Keeping this file small keeps the amount of ZougCloud
//! code sitting inside upstream's `src/` to a minimum.

use std::{fmt::Display, path::PathBuf, sync::Arc};

use log::info;
use process::{
    error::ProcessError,
    resolve::{ResolvedLaunch, resolve_launch_target, resolve_launch_targets},
};
use serde::Serialize;
use serde_with::SerializeDisplay;
use steam::{
    ShortcutRecord, ShortcutRequest, SteamError, SteamInstall, SteamUser, find_shortcut,
    locate_steam, remove_shortcut, run_game_id, upsert_shortcut,
};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

// Debug is hand-written below: upstream's ProcessError implements Display but
// not Debug, so it cannot be derived here.
#[derive(SerializeDisplay, Clone)]
pub enum SteamCommandError {
    Steam(SteamError),
    Process(ProcessError),
    /// The chosen launch option resolves to a file that is not on disk. A
    /// shortcut pointing at nothing would fail silently inside Steam.
    NoExecutable(String),
    Opener(Arc<tauri_plugin_opener::Error>),
}

impl Display for SteamCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SteamCommandError::Steam(e) => write!(f, "{e}"),
            SteamCommandError::Process(e) => write!(f, "{e}"),
            SteamCommandError::NoExecutable(p) => {
                write!(f, "The game executable could not be found: {p}")
            }
            SteamCommandError::Opener(e) => write!(f, "Could not open Steam: {e}"),
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

#[tauri::command]
pub fn steam_game_status(
    game_id: String,
    app_name: String,
    account_id: Option<u32>,
) -> Result<SteamGameStatus, SteamCommandError> {
    let launches = resolve_launch_targets(&game_id).unwrap_or_default();

    let install = match locate_steam() {
        Ok(install) => install,
        Err(SteamError::NotInstalled | SteamError::NoUsers) => {
            return Ok(SteamGameStatus {
                install: None,
                shortcut: None,
                launches,
            });
        }
        Err(e) => return Err(e.into()),
    };

    // A shortcut is looked up against whichever launch option currently
    // resolves; matching also falls back to the game name, so a shortcut added
    // for a different launch option is still found.
    let shortcut = {
        let user = user_for(&install, account_id)?;
        launches
            .iter()
            .find_map(|launch| find_shortcut(user, &app_name, &launch.exe).ok().flatten())
    };

    Ok(SteamGameStatus {
        install: Some(install),
        shortcut,
        launches,
    })
}

#[tauri::command]
pub fn steam_add_shortcut(
    game_id: String,
    app_name: String,
    launch_index: usize,
    account_id: Option<u32>,
) -> Result<ShortcutRecord, SteamCommandError> {
    let install = locate_steam()?;

    // Steam rewrites shortcuts.vdf from memory when it exits, so a write made
    // now would be silently discarded. Refusing is the only honest option.
    if install.running {
        return Err(SteamError::SteamRunning.into());
    }

    let user = user_for(&install, account_id)?;
    let launch = resolve_launch_target(&game_id, launch_index)?;

    if !launch.exists {
        return Err(SteamCommandError::NoExecutable(
            launch.exe.display().to_string(),
        ));
    }

    let request = ShortcutRequest {
        app_name,
        exe: launch.exe.clone(),
        start_dir: launch.working_dir.clone(),
        launch_options: launch.args.join(" "),
        icon: None,
    };

    let record = upsert_shortcut(user, &request)?;
    info!(
        "added game {} to Steam account {} as app {}",
        game_id, user.account_id, record.app_id
    );
    Ok(record)
}

#[tauri::command]
pub fn steam_remove_shortcut(
    app_id: u32,
    account_id: Option<u32>,
) -> Result<(), SteamCommandError> {
    let install = locate_steam()?;

    if install.running {
        return Err(SteamError::SteamRunning.into());
    }

    let user = user_for(&install, account_id)?;
    remove_shortcut(user, app_id)?;
    info!("removed Steam shortcut {app_id}");
    Ok(())
}

/// Open the game's page in Steam. Safe to call while Steam is running -- it
/// only follows a URL and never touches shortcuts.vdf.
#[tauri::command]
pub fn steam_open_shortcut(app_id: u32, app_handle: AppHandle) -> Result<(), SteamCommandError> {
    let url = format!("steam://nav/games/details/{}", run_game_id(app_id));
    app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| SteamCommandError::Opener(Arc::new(e)))
}

/// Where this account's custom artwork lives. Used by ZC-005.
#[tauri::command]
pub fn steam_grid_dir(account_id: Option<u32>) -> Result<PathBuf, SteamCommandError> {
    let install = locate_steam()?;
    Ok(user_for(&install, account_id)?.grid_dir())
}
