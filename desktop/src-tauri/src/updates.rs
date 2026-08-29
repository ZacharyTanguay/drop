use std::sync::nonpoison::Mutex;

use async_trait::async_trait;
use client::{app_state::AppState, app_status::AppStatus};
use database::{
    GameDownloadStatus, GameVersion, borrow_db_checked, borrow_db_mut_checked,
};
// ZOUGCLOUD(ZC-002): `::games` disambiguates the workspace crate from `crate::games`.
use ::games::{library::push_game_update, state::GameStatusManager};
use log::warn;
use process::PROCESS_MANAGER;
use remote::utils::DROP_APP_HANDLE;
use tauri::Manager;

use crate::{
    games::{VersionDownloadOption, fetch_game_version_options},
    scheduler::ScheduleTask,
};

pub struct GameUpdater {
    no_internet: bool,
}

impl GameUpdater {
    pub fn new() -> Self {
        GameUpdater { no_internet: false }
    }
}

/*
This implementation is kinda inefficient because we can't hold the locks across await boundaries,
which means we constantly lock and unlock certain objects. It doesn't matter though, because this
doesn't have to be fast.
*/
#[async_trait]
impl ScheduleTask for GameUpdater {
    fn timeframe(&mut self) -> usize {
        // ZOUGCLOUD(ZC-002): upstream polls every 30 minutes, which means a version
        // published on the server can stay invisible for half an hour. 5 minutes is
        // responsive enough for our members while staying gentle on the server: one
        // `fetch_game_version_options` per *installed* game with updates enabled.
        if self.no_internet { 2 } else { 5 }
    }

    async fn call(&mut self) -> Result<(), anyhow::Error> {
        let app_handle = DROP_APP_HANDLE.lock().await;
        let app_handle = app_handle
            .as_ref()
            .ok_or(anyhow::anyhow!("game update task ran before setup"))?;
        let state = app_handle.state::<Mutex<AppState>>();
        {
            let state_lock = state.lock();
            if state_lock.status == AppStatus::Offline {
                self.no_internet = true;
                return Ok(());
            };
        };

        self.no_internet = false;

        let to_check: Vec<GameVersion> = {
            let db_lock = borrow_db_checked();

            

            db_lock
                .applications
                .game_statuses
                .values()
                .map(|v| match v {
                    GameDownloadStatus::Installed { version_id, .. } => Some(version_id),
                    _ => None,
                })
                .map(|v| {
                    v.and_then(|version_id| db_lock.applications.game_versions.get(version_id))
                })
                .filter(|v| {
                    v.map(|v| v.user_configuration.enable_updates)
                        .unwrap_or(false)
                })
                .map(|v| v.cloned().unwrap())
                .collect()
        };

        for version in to_check {
            let version_options =
                match fetch_game_version_options(version.game_id.clone(), state.clone()).await {
                    Ok(v) => v,
                    Err(err) => {
                        warn!(
                            "failed to check for update for game id {}: {:?}",
                            version.game_id, err
                        );
                        continue;
                    }
                };

            let process_manager_lock = PROCESS_MANAGER.lock();
            let valid_options: Vec<VersionDownloadOption> = version_options
                .into_iter()
                .filter(|v| process_manager_lock.valid_platform(&v.platform))
                .collect();

            let latest = match valid_options.first() {
                Some(v) => v,
                None => {
                    warn!("found no versions for game id: {}", version.game_id);
                    continue;
                }
            };
            let has_update = latest.version_id != version.version_id;

            let update_state_changed = {
                let mut db_lock = borrow_db_mut_checked();
                let game_status = db_lock
                    .applications
                    .game_statuses
                    .get_mut(&version.game_id)
                    .ok_or(anyhow::anyhow!(""))?;

                match game_status {
                    GameDownloadStatus::Installed {
                        update_available, ..
                    } => {
                        let changed = *update_available != has_update;
                        *update_available = has_update;
                        changed
                    }
                    _ => false,
                }
            };

            // ZOUGCLOUD(ZC-002): upstream writes `update_available` into the database
            // and stops there. The frontend keeps every game in a module-level registry
            // (`main/composables/game.ts`) that is only ever refreshed by an
            // `update_game/{id}` event, so the library keeps rendering "Up to date"
            // until the whole app is restarted. Emit the event upstream forgot — and
            // only on an actual transition, so a poll that finds nothing new stays
            // silent instead of waking the UI every cycle.
            if update_state_changed {
                let status = {
                    let db_lock = borrow_db_checked();
                    GameStatusManager::fetch_state(&version.game_id, &db_lock)
                };
                push_game_update(app_handle, &version.game_id, Some(version.clone()), status);
            }
        }

        Ok(())
    }
}
