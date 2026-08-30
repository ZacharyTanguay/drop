use std::sync::nonpoison::Mutex;

use bitcode::{Decode, Encode};
use database::{
    DownloadableMetadata, GameDownloadStatus, borrow_db_checked, borrow_db_mut_checked,
    models::data::{InstalledGameType, UserConfiguration}, platform::Platform,
};
use games::{
    collections::collection::Collection,
    downloads::error::LibraryError,
    library::{FetchGameStruct, Game, get_current_meta, uninstall_game_logic},
    state::{GameStatusManager, GameStatusWithTransient},
};
use log::{debug, warn};
use process::PROCESS_MANAGER;
use remote::{
    auth::generate_authorization_header,
    cache::{cache_object, cache_object_db, get_cached_object},
    error::{DropServerError, RemoteAccessError},
    offline,
    requests::generate_url,
    utils::DROP_CLIENT_ASYNC,
};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{AppState, collections::fetch_collections};

#[tauri::command]
pub async fn fetch_library(
    state: tauri::State<'_, Mutex<AppState>>,
    app_handle: AppHandle,
    hard_refresh: Option<bool>,
) -> Result<FetchLibraryResponse, RemoteAccessError> {
    let response = offline!(
        state,
        fetch_library_logic,
        fetch_library_logic_offline,
        state,
        app_handle,
        hard_refresh
    )
    .await?;

    // ZOUGCLOUD(ZC-011): filter here, at the command boundary, rather than
    // inside the logic. The cache underneath keeps the full library, so an
    // access change takes effect on the next read without refetching from the
    // server — and the same filter covers the offline path for free.
    Ok(response.filtered_for_viewer())
}

#[derive(Encode, Decode, Serialize)]
pub struct FetchLibraryResponse {
    library: Vec<Game>,
    collections: Vec<Collection>,
    other: Vec<Game>,
    missing: Vec<Game>,
}

pub async fn fetch_library_logic(
    state: tauri::State<'_, Mutex<AppState>>,
    app_handle: AppHandle,
    hard_fresh: Option<bool>,
) -> Result<FetchLibraryResponse, RemoteAccessError> {
    let do_hard_refresh = hard_fresh.unwrap_or(false);
    if !do_hard_refresh && let Ok(library) = get_cached_object("library") {
        return Ok(library);
    }

    let response = generate_url(&["/api/v1/client/user/library"], &[])?;
    let auth_header = generate_authorization_header();
    let response = DROP_CLIENT_ASYNC
        .get(response)
        .header("Authorization", auth_header)
        .send()
        .await?;

    if response.status() != 200 {
        let err = response.json().await.unwrap_or(DropServerError {
            status_code: 500,
            status_message: "Server Error".to_owned(),
            message: "Invalid response from server.".to_owned(),
        });
        warn!("{err:?}");
        return Err(RemoteAccessError::InvalidResponse(err));
    }

    let library: Vec<Game> = response.json().await?;
    let collections = fetch_collections(state, hard_fresh).await?;

    let mut all_games = library.clone();
    all_games.extend(
        collections
            .iter()
            .flat_map(|v| v.entries.iter().map(|v| v.game.clone())),
    );

    let installed_metas = {
        let mut db_handle = borrow_db_mut_checked();

        for game in &all_games {
            if !db_handle.applications.game_statuses.contains_key(game.id()) {
                db_handle
                    .applications
                    .game_statuses
                    .insert(game.id().clone(), GameDownloadStatus::Remote {});
            }
            cache_object_db(&format!("game/{}", game.id), game, &db_handle)?;
        }

        db_handle
            .applications
            .installed_game_version
            .values()
            .cloned()
            .collect::<Vec<DownloadableMetadata>>()
    };

    // Add games that are installed but no longer in library
    let mut other = Vec::new();
    let mut missing = Vec::new();
    for meta in installed_metas {
        if all_games.iter().any(|e| *e.id() == meta.id) {
            continue;
        }
        // We should always have a cache of the object
        // Pass db_handle because otherwise we get a gridlock
        let game = match get_cached_object::<Game>(&meta.id.clone()) {
            Ok(game) => game,
            Err(err) => {
                warn!(
                    "{} is installed, but encountered error fetching its error: {}.",
                    meta.id, err
                );
                /*
                 * We can't return a dummy object here because it needs to be in the cache to work
                 * So we uninstall the game so we don't "lose" it
                 */
                uninstall_game_logic(meta.clone(), &app_handle);
                continue;
            }
        };
        if game.game_type == "Game" {
            missing.push(game);
        } else {
            other.push(game);
        }
    }

    let response = FetchLibraryResponse {
        library,
        collections,
        other,
        missing,
    };

    cache_object("library", &response)?;

    Ok(response)
}
pub async fn fetch_library_logic_offline(
    _state: tauri::State<'_, Mutex<AppState>>,
    _app_handle: AppHandle,
    _hard_refresh: Option<bool>,
) -> Result<FetchLibraryResponse, RemoteAccessError> {
    let mut response: FetchLibraryResponse = get_cached_object("library")?;

    let db_handle = borrow_db_checked();

    let retain_filter = |game: &Game| {
        matches!(
            &db_handle
                .applications
                .game_statuses
                .get(game.id())
                .unwrap_or(&GameDownloadStatus::Remote {}),
            GameDownloadStatus::Installed {
                install_type: InstalledGameType::Installed | InstalledGameType::SetupRequired,
                ..
            }
        )
    };

    response.library.retain(retain_filter);
    response.other.retain(retain_filter);
    response.missing.retain(retain_filter);
    response
        .collections
        .iter_mut()
        .for_each(|k| k.entries.retain(|object| retain_filter(&object.game)));

    Ok(response)
}
pub async fn fetch_game_logic(
    id: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<FetchGameStruct, RemoteAccessError> {
    let version = {
        let db_lock = borrow_db_checked();

        let metadata_option = db_lock.applications.installed_game_version.get(&id);

        match metadata_option {
            None => None,
            Some(metadata) => db_lock
                .applications
                .game_versions
                .get(&metadata.version)
                .cloned(),
        }
    };

    let game = match get_cached_object::<Game>(&format!("game/{}", id)) {
        Ok(value) => value,
        Err(_) => {
            let client = DROP_CLIENT_ASYNC.clone();
            let response = generate_url(&["/api/v1/client/game", &id], &[])?;
            let response = client
                .get(response)
                .header("Authorization", generate_authorization_header())
                .send()
                .await?;

            if response.status() == 404 {
                let offline_fetch = fetch_game_logic_offline(id.clone(), state).await;
                if let Ok(fetch_data) = offline_fetch {
                    return Ok(fetch_data);
                }

                return Err(RemoteAccessError::GameNotFound(id));
            }
            if response.status() != 200 {
                let err = response.json().await?;
                warn!("{err:?}");
                return Err(RemoteAccessError::InvalidResponse(err));
            }

            let game: Game = response.json().await?;
            game
        }
    };

    let mut db_handle = borrow_db_mut_checked();

    db_handle
        .applications
        .game_statuses
        .entry(id.clone())
        .or_insert(GameDownloadStatus::Remote {});

    let status = GameStatusManager::fetch_state(&id, &db_handle);

    drop(db_handle);

    let data = FetchGameStruct::new(game.clone(), status, version);

    cache_object(&id, &game)?;

    Ok(data)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionDownloadOptionRequiredContent {
    game_id: String,
    version_id: String,
    name: String,
    icon_object_id: String,
    short_description: String,
    size: GameSize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDownloadOption {
    pub game_id: String,
    pub version_id: String,
    display_name: Option<String>,
    version_path: String,
    pub platform: Platform,
    size: GameSize,
    required_content: Vec<VersionDownloadOptionRequiredContent>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSize {
    install_size: usize,
    download_size: usize,
}

pub async fn fetch_game_version_options_logic(
    game_id: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<VersionDownloadOption>, RemoteAccessError> {
    let client = DROP_CLIENT_ASYNC.clone();

    let previous_id = borrow_db_checked()
        .applications
        .installed_game_version
        .get(&game_id)
        .map(|v| v.version.clone());

    let response = generate_url(
        &["/api/v1/client/game", &game_id, "versions"],
        &[("previous", &previous_id.unwrap_or(String::new()))],
    )?;
    let response = client
        .get(response)
        .header("Authorization", generate_authorization_header())
        .send()
        .await?;

    if response.status() != 200 {
        let err = response.json().await?;
        warn!("{err:?}");
        return Err(RemoteAccessError::InvalidResponse(err));
    }

    let data: Vec<VersionDownloadOption> = response.json().await?;

    let state_lock = state.lock();
    let process_manager_lock = PROCESS_MANAGER.lock();
    let data: Vec<VersionDownloadOption> = data
        .into_iter()
        .filter(|v| process_manager_lock.valid_platform(&v.platform))
        .collect();
    //data.dedup_by_key(|v| v.platform);
    drop(process_manager_lock);
    drop(state_lock);

    Ok(data)
}

pub async fn fetch_game_logic_offline(
    id: String,
    _state: tauri::State<'_, Mutex<AppState>>,
) -> Result<FetchGameStruct, RemoteAccessError> {
    let db_handle = borrow_db_checked();
    let metadata_option = db_handle.applications.installed_game_version.get(&id);
    let version = match metadata_option {
        None => None,
        Some(metadata) => db_handle
            .applications
            .game_versions
            .get(&metadata.version)
            .cloned(),
    };

    let status = GameStatusManager::fetch_state(&id, &db_handle);
    let game = get_cached_object::<Game>(&id)?;

    drop(db_handle);

    Ok(FetchGameStruct::new(game, status, version))
}

#[tauri::command]
pub async fn fetch_game(
    game_id: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<FetchGameStruct, RemoteAccessError> {
    // ZOUGCLOUD(ZC-011): a direct route to a game the member may not have.
    // GameNotFound is the honest answer from their point of view, and it is an
    // existing variant, so this needs no upstream change. The error page (ZC-012)
    // classifies it as not-found and offers Back to Library rather than a Retry
    // that could only fail the same way.
    if !::access::ACCESS.is_accessible(&game_id) {
        return Err(RemoteAccessError::GameNotFound(game_id));
    }

    offline!(
        state,
        fetch_game_logic,
        fetch_game_logic_offline,
        game_id,
        state
    )
    .await
}

#[tauri::command]
pub fn fetch_game_status(id: String) -> GameStatusWithTransient {
    let db_handle = borrow_db_checked();
    GameStatusManager::fetch_state(&id, &db_handle)
}

#[tauri::command]
pub fn uninstall_game(game_id: String, app_handle: AppHandle) -> Result<(), LibraryError> {
    let meta = match get_current_meta(&game_id) {
        Some(data) => data,
        None => return Err(LibraryError::MetaNotFound(game_id)),
    };
    uninstall_game_logic(meta, &app_handle);

    Ok(())
}

#[tauri::command]
pub async fn fetch_game_version_options(
    game_id: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<VersionDownloadOption>, RemoteAccessError> {
    fetch_game_version_options_logic(game_id, state).await
}

#[tauri::command]
pub fn update_game_configuration(
    game_id: String,
    options: UserConfiguration,
) -> Result<(), LibraryError> {
    let mut handle = borrow_db_mut_checked();
    let installed_version = handle
        .applications
        .installed_game_version
        .get(&game_id)
        .ok_or(LibraryError::MetaNotFound(game_id))?;

    let _id = installed_version.id.clone();
    let version = installed_version.version.clone();

    let mut existing_configuration = handle
        .applications
        .game_versions
        .get(&version)
        .unwrap()
        .clone();

    existing_configuration.user_configuration = options;

    handle
        .applications
        .game_versions
        .insert(version.to_string(), existing_configuration);

    Ok(())
}

// ZOUGCLOUD(ZC-011): applying the access rules to the native surfaces.
//
// Every surface reads from one of two commands -- `fetch_library` (which also
// feeds search, browse and the collection lists) and `fetch_game` -- so those
// are the only two places that need to filter. The rule itself lives in the
// `access` crate; nothing here re-implements it.

impl FetchLibraryResponse {
    /// Drop everything the signed-in member may not have.
    ///
    /// Filtering the response rather than the cache is deliberate: the cache
    /// keeps the full library, so an access change applies on the next read
    /// without a round trip to the server, and a member who loses access does
    /// not need their local data rewritten.
    fn filtered_for_viewer(self) -> Self {
        self.filtered_with(|id| ::access::ACCESS.is_accessible(id))
    }

    /// The filtering itself, with the decision injected so it can be tested
    /// without standing up the global access state.
    fn filtered_with<F>(mut self, accessible: F) -> Self
    where
        F: Fn(&str) -> bool,
    {
        let before = self.library.len() + self.other.len() + self.missing.len();

        self.library.retain(|game| accessible(game.id()));
        self.other.retain(|game| accessible(game.id()));
        self.missing.retain(|game| accessible(game.id()));

        // Collections carry their own copies of games, so missing this would
        // leave a hidden game visible in a collection while it is gone from the
        // library — the kind of inconsistency that makes filtering look broken.
        for collection in &mut self.collections {
            collection.entries.retain(|entry| accessible(&entry.game_id));
        }

        let after = self.library.len() + self.other.len() + self.missing.len();
        if before != after {
            debug!(
                "access filter hid {} game(s) from the library",
                before - after
            );
        }

        self
    }
}

/// What the UI needs to render a game's access state.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameAccess {
    pub decision: ::access::AccessDecision,
    pub allowed: bool,
    /// True only for a gated game a Custom member has not been granted. Free
    /// games, All members and already-granted games never show interest.
    pub offers_interest: bool,
    /// Minor units, so the frontend renders without ever touching a float.
    pub price_amount_minor: Option<i64>,
    pub price_currency: Option<String>,
    /// Pre-formatted for display, or None when no price is configured — which
    /// is not the same as free.
    pub price_display: Option<String>,
}

#[tauri::command]
pub fn fetch_game_access(game_id: String) -> GameAccess {
    let decision = ::access::ACCESS.decide(&game_id);
    let price = ::access::ACCESS.price(&game_id);

    GameAccess {
        decision,
        allowed: decision.is_allowed(),
        offers_interest: decision.offers_interest(),
        price_amount_minor: price.as_ref().map(|p| p.amount_minor),
        price_currency: price.as_ref().map(|p| p.currency.clone()),
        price_display: price.as_ref().map(::access::format_price),
    }
}

/// Whether the signed-in user is the ZougCloud admin, for showing admin-only
/// surfaces. UX only — it decides what is rendered, never what is permitted.
#[tauri::command]
pub fn zougcloud_is_admin() -> bool {
    ::access::ACCESS.viewer_is_admin()
}

// ZOUGCLOUD(ZC-011): the filter is four `retain` calls, and the one most
// easily forgotten is the collections -- a game hidden from the library but
// still listed in a collection looks like the filter simply does not work.
#[cfg(test)]
mod access_filter_tests {
    use super::*;
    use ::games::collections::collection::CollectionObject;

    const ALLOWED: &str = "allowed-game";
    const DENIED: &str = "denied-game";

    fn game(id: &str) -> Game {
        Game {
            id: id.to_owned(),
            ..Default::default()
        }
    }

    fn entry(id: &str) -> CollectionObject {
        CollectionObject {
            game_id: id.to_owned(),
            game: game(id),
            ..Default::default()
        }
    }

    fn response() -> FetchLibraryResponse {
        // Collection's other fields are private to its crate, so it is built
        // and then populated through the one public field.
        let mut collection = Collection::default();
        collection.entries = vec![entry(ALLOWED), entry(DENIED)];

        FetchLibraryResponse {
            library: vec![game(ALLOWED), game(DENIED)],
            other: vec![game(ALLOWED), game(DENIED)],
            missing: vec![game(ALLOWED), game(DENIED)],
            collections: vec![collection],
        }
    }

    #[test]
    fn a_denied_game_disappears_from_every_surface() {
        let filtered = response().filtered_with(|id| id == ALLOWED);

        assert_eq!(filtered.library.len(), 1);
        assert_eq!(filtered.library[0].id, ALLOWED);
        assert_eq!(filtered.other.len(), 1);
        assert_eq!(filtered.missing.len(), 1);

        // The easy one to miss.
        assert_eq!(filtered.collections[0].entries.len(), 1);
        assert_eq!(filtered.collections[0].entries[0].game_id, ALLOWED);
    }

    #[test]
    fn allowing_everything_changes_nothing() {
        // The admin path: the response passes through untouched.
        let filtered = response().filtered_with(|_| true);

        assert_eq!(filtered.library.len(), 2);
        assert_eq!(filtered.other.len(), 2);
        assert_eq!(filtered.missing.len(), 2);
        assert_eq!(filtered.collections[0].entries.len(), 2);
    }

    #[test]
    fn denying_everything_leaves_an_empty_library_not_a_broken_one() {
        // What a Custom member sees before any policy is configured. Empty is
        // the intended result, and the collection survives as an empty shell
        // rather than the response becoming malformed.
        let filtered = response().filtered_with(|_| false);

        assert!(filtered.library.is_empty());
        assert!(filtered.other.is_empty());
        assert!(filtered.missing.is_empty());
        assert_eq!(filtered.collections.len(), 1);
        assert!(filtered.collections[0].entries.is_empty());
    }
}
