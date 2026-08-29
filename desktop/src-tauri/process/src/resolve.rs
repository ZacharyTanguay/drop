//! ZOUGCLOUD(ZC-004): resolve a launch option to a concrete executable without
//! running it.
//!
//! The Steam integration needs the same answer `launch_process` computes -- which
//! file to run, with which arguments, from which directory -- but as data rather
//! than as a spawned child.
//!
//! It deliberately stops before `create_launch_process`. That step wraps the
//! command in `cmd.exe` or PowerShell when appropriate, which is right for
//! launching and wrong for a Steam shortcut: Steam must point at the game
//! itself, or the overlay attaches to a shell and playtime is recorded for the
//! wrapper instead of the game.
//!
//! Reusing `ParsedCommand` keeps this consistent with ZC-003, so an executable
//! whose name contains spaces resolves identically here and at launch time.

use std::path::PathBuf;

use database::{
    GameDownloadStatus, borrow_db_checked, models::data::InstalledGameType,
};
use log::debug;
use serde::Serialize;

use crate::{error::ProcessError, parser::ParsedCommand};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLaunch {
    /// Index into the platform-filtered launch list, so callers can round-trip
    /// a user's choice.
    pub index: usize,
    pub name: String,
    /// Absolute path to the game executable.
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    /// False when the executable is not on disk. Surfaced rather than treated
    /// as an error so the UI can explain itself.
    pub exists: bool,
}

/// Resolve every launch option this game offers on the current platform.
pub fn resolve_launch_targets(game_id: &str) -> Result<Vec<ResolvedLaunch>, ProcessError> {
    let db_lock = borrow_db_checked();

    let meta = db_lock
        .applications
        .installed_game_version
        .get(game_id)
        .cloned()
        .ok_or(ProcessError::NotInstalled)?;

    let game_status = db_lock
        .applications
        .game_statuses
        .get(game_id)
        .ok_or(ProcessError::NotInstalled)?;

    let install_dir = match game_status {
        GameDownloadStatus::Installed {
            install_dir,
            install_type: InstalledGameType::Installed | InstalledGameType::SetupRequired,
            ..
        } => install_dir.clone(),
        _ => return Err(ProcessError::NotInstalled),
    };

    let game_version = db_lock
        .applications
        .game_versions
        .get(&meta.version)
        .ok_or(ProcessError::InvalidVersion)?;

    let base = PathBuf::from(&install_dir);

    let resolved = game_version
        .launches
        .iter()
        .filter(|launch| launch.platform == meta.target_platform)
        .enumerate()
        .filter_map(|(index, launch)| {
            // An emulator launch runs the emulator with the game as an argument.
            // Resolving that to a single executable would produce a shortcut
            // that points at the ROM, so it is skipped rather than guessed at.
            if launch.emulator.is_some() {
                debug!(
                    "skipping emulator launch '{}' for Steam resolution",
                    launch.name
                );
                return None;
            }

            let mut parsed = ParsedCommand::parse(launch.command.clone()).ok()?;
            parsed.coalesce_unquoted_command(&base);
            parsed.make_command_absolute_if_local(&base);

            let exe = PathBuf::from(&parsed.command);
            Some(ResolvedLaunch {
                index,
                name: launch.name.clone(),
                exists: exe.is_file(),
                exe,
                args: parsed.args.clone(),
                working_dir: base.clone(),
            })
        })
        .collect();

    Ok(resolved)
}

/// Resolve a single launch option by its index in the platform-filtered list.
pub fn resolve_launch_target(
    game_id: &str,
    launch_index: usize,
) -> Result<ResolvedLaunch, ProcessError> {
    resolve_launch_targets(game_id)?
        .into_iter()
        .find(|launch| launch.index == launch_index)
        .ok_or(ProcessError::InvalidID)
}
