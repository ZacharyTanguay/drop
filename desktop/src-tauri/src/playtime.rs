//! ZOUGCLOUD(ZC-008): playtime commands and the external process watcher.
//!
//! The accounting itself lives in the `playtime` crate. This module is the glue
//! that binds it to Drop: a Tauri command for the UI, and a background task that
//! notices games started outside Drop (from a Steam shortcut, say) so their time
//! is counted too.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use database::{GameDownloadStatus, borrow_db_checked, db::DATA_ROOT_DIR};
use log::{debug, info, warn};
use playtime::{PLAYTIME, SessionOwner, format_last_played, format_playtime};
use process::resolve::resolve_launch_targets;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// How often the watcher looks at the process list.
///
/// Deliberately coarse. This runs for the whole life of the app, usually while
/// Drop sits in the tray, so it must be cheap; a few seconds of imprecision at
/// the edges of a session does not matter for a playtime counter. It also bounds
/// how much a crash can lose, since the heartbeat rides on the same tick.
const POLL_INTERVAL: Duration = Duration::from_secs(7);

/// How often the list of executables to watch is rebuilt. Resolving launch
/// targets touches the database and the filesystem, so it is not worth doing
/// every tick — games are not installed that often.
const TARGET_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Emitted when a session opened by the watcher closes, so a game page that is
/// already open picks up the new total. Drop-launched sessions do not need this:
/// upstream already pushes `update_game/{id}` when the process exits.
const PLAYTIME_EVENT: &str = "zougcloud:playtime-updated";

pub fn zougcloud_dir() -> PathBuf {
    DATA_ROOT_DIR.join("zougcloud")
}

pub fn playtime_path() -> PathBuf {
    zougcloud_dir().join("playtime.json")
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlaytimeSummary {
    pub total_seconds: u64,
    pub last_played_at: Option<i64>,
    /// Ready-to-render text, or `None` when the game has never been played so
    /// the UI can omit the element entirely.
    pub display: Option<String>,
    /// Secondary "Last played …" line.
    pub last_played_display: Option<String>,
    /// A session is open right now, so `total_seconds` is not yet final.
    pub active: bool,
}

#[tauri::command]
pub fn fetch_playtime(game_id: String) -> PlaytimeSummary {
    let now = playtime::now();
    let tracker = PLAYTIME.lock();
    let entry = tracker.playtime(&game_id);

    PlaytimeSummary {
        total_seconds: entry.total_playtime_seconds,
        last_played_at: entry.last_played_at,
        display: format_playtime(entry.total_playtime_seconds),
        last_played_display: format_last_played(entry.last_played_at, now),
        active: tracker.is_active(&game_id),
    }
}

/// Executables worth watching, keyed by lowercased absolute path.
///
/// Only **direct executables** are watched. A launch command that goes through
/// a `.bat`, `cmd`, PowerShell or an emulator spawns a process we did not name
/// and cannot reliably attribute to a game, so those are skipped rather than
/// guessed at — see docs/ZOUGCLOUD-PATCHES.md for the limitation.
fn watch_targets() -> HashMap<String, String> {
    let installed: Vec<String> = {
        let db = borrow_db_checked();
        db.applications
            .game_statuses
            .iter()
            .filter(|(_, status)| matches!(status, GameDownloadStatus::Installed { .. }))
            .map(|(game_id, _)| game_id.clone())
            .collect()
    };

    let mut targets = HashMap::new();
    for game_id in installed {
        let Ok(launches) = resolve_launch_targets(&game_id) else {
            continue;
        };
        for launch in launches {
            if !launch.exists {
                continue;
            }
            let is_executable = launch
                .exe
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
            if !is_executable {
                debug!(
                    "not watching {} for {game_id}: not a direct executable",
                    launch.exe.display()
                );
                continue;
            }
            targets.insert(launch.exe.to_string_lossy().to_lowercase(), game_id.clone());
        }
    }

    targets
}

/// Background task: notice games started outside Drop, and keep the heartbeat
/// of every open session fresh.
///
/// The double-counting guarantee is not enforced here but in the tracker:
/// `begin_session` refuses when a session is already open, and `end_session`
/// only closes a session of the matching owner. So a game Drop launched itself
/// is simply observed by this loop, never taken over.
pub async fn playtime_watcher(app_handle: AppHandle) -> ! {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

    let mut system = System::new_with_specifics(RefreshKind::nothing());
    let refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::Always);

    let mut targets = watch_targets();
    let mut last_target_refresh = std::time::Instant::now();
    let mut interval = tokio::time::interval(POLL_INTERVAL);

    info!("playtime watcher started, watching {} target(s)", targets.len());

    loop {
        interval.tick().await;

        if last_target_refresh.elapsed() >= TARGET_REFRESH_INTERVAL {
            targets = watch_targets();
            last_target_refresh = std::time::Instant::now();
        }

        if targets.is_empty() {
            continue;
        }

        system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);

        // Which watched games have a live process right now.
        let mut running: HashMap<&str, ()> = HashMap::new();
        for process in system.processes().values() {
            let Some(exe) = process.exe() else { continue };
            let key = exe.to_string_lossy().to_lowercase();
            if let Some(game_id) = targets.get(&key) {
                running.insert(game_id.as_str(), ());
            }
        }

        let now = playtime::now();
        let mut ended: Vec<String> = Vec::new();

        {
            let mut tracker = PLAYTIME.lock();

            for game_id in targets.values() {
                let is_running = running.contains_key(game_id.as_str());

                if is_running {
                    // A no-op when Drop already owns the session, which is
                    // exactly what stops the double count.
                    if tracker.begin_session(game_id, SessionOwner::Watcher, now) {
                        info!("watcher saw {game_id} start outside Drop");
                    }
                    tracker.heartbeat(game_id, now);
                } else if tracker.active_owner(game_id) == Some(SessionOwner::Watcher)
                    && tracker
                        .end_session(game_id, SessionOwner::Watcher, now)
                        .is_some()
                {
                    ended.push(game_id.clone());
                }
            }
        }

        for game_id in ended {
            if let Err(e) = app_handle.emit(PLAYTIME_EVENT, &game_id) {
                warn!("could not emit playtime update for {game_id}: {e}");
            }
        }
    }
}
