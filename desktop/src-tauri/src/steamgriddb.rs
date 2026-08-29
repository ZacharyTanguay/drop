//! ZOUGCLOUD(ZC-005): optional SteamGridDB artwork source.
//!
//! Entirely optional. SteamGridDB requires a personal API key, and we will not
//! ship one: a hardcoded key would be committed to a public AGPL repository,
//! shared by every member, and revoked the moment anyone noticed. So the key is
//! supplied by the user, stored locally, and the whole feature degrades to
//! Drop's own artwork when it is absent.
//!
//! The key is kept in its own file under Drop's data directory rather than in
//! `drop.db`. That keeps it out of the database blob (and therefore out of
//! backups and crash dumps of it), and avoids adding a field to an upstream
//! model, which would be one more thing to reconcile at every rebase.

use std::path::PathBuf;

use database::db::DATA_ROOT_DIR;
use log::{debug, warn};
use serde::Deserialize;

use crate::steam::SteamCommandError;

const API: &str = "https://www.steamgriddb.com/api/v2";

pub fn key_path() -> PathBuf {
    DATA_ROOT_DIR.join("zougcloud").join("steamgriddb.key")
}

/// The stored key, if any. Never sent to the frontend -- see `is_configured`.
pub fn load_key() -> Option<String> {
    let text = std::fs::read_to_string(key_path()).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub fn is_configured() -> bool {
    load_key().is_some()
}

/// Store or clear the key. An empty string clears it.
pub fn save_key(key: Option<&str>) -> Result<(), std::io::Error> {
    let path = key_path();
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, key)
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    }
}

#[derive(Deserialize)]
struct SgdbEnvelope<T> {
    success: bool,
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

#[derive(Deserialize)]
struct SgdbGame {
    id: u32,
}

#[derive(Deserialize)]
struct SgdbAsset {
    url: String,
}

/// Which SteamGridDB endpoint and dimensions serve each Steam artwork slot.
///
/// The dimensions matter: Steam stretches whatever it finds, and a wide capsule
/// dropped into the portrait slot looks broken. Asking the API for the right
/// shape is cheaper than validating it afterwards.
pub fn endpoint_for(kind: steam::ArtworkKind) -> (&'static str, Option<&'static str>) {
    match kind {
        steam::ArtworkKind::Portrait => ("grids", Some("600x900")),
        steam::ArtworkKind::Capsule => ("grids", Some("920x430")),
        steam::ArtworkKind::Hero => ("heroes", None),
        steam::ArtworkKind::Logo => ("logos", None),
        steam::ArtworkKind::Icon => ("icons", None),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Find the SteamGridDB game id for a title. `None` simply means no artwork.
pub async fn find_game(key: &str, name: &str) -> Option<u32> {
    let url = format!("{API}/search/autocomplete/{}", urlencoding::encode(name));
    let response = client()
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .inspect_err(|e| warn!("SteamGridDB search failed: {e}"))
        .ok()?;

    if !response.status().is_success() {
        warn!("SteamGridDB search returned {}", response.status());
        return None;
    }

    let body: SgdbEnvelope<SgdbGame> = response.json().await.ok()?;
    if !body.success {
        return None;
    }

    let id = body.data.first().map(|game| game.id);
    debug!("SteamGridDB matched {name:?} to {id:?}");
    id
}

/// Download the best asset SteamGridDB has for one slot.
///
/// Every failure returns `None` rather than an error: artwork is a nicety, and
/// a SteamGridDB outage must never stop a game being added to Steam.
pub async fn fetch_asset(key: &str, game_id: u32, kind: steam::ArtworkKind) -> Option<Vec<u8>> {
    let (endpoint, dimensions) = endpoint_for(kind);
    let mut url = format!("{API}/{endpoint}/game/{game_id}");
    if let Some(dimensions) = dimensions {
        url.push_str(&format!("?dimensions={dimensions}"));
    }

    let response = client().get(&url).bearer_auth(key).send().await.ok()?;
    if !response.status().is_success() {
        debug!("SteamGridDB {endpoint} returned {}", response.status());
        return None;
    }

    let body: SgdbEnvelope<SgdbAsset> = response.json().await.ok()?;
    let asset = body.data.first()?;

    let image = client().get(&asset.url).send().await.ok()?;
    if !image.status().is_success() {
        return None;
    }

    let bytes = image.bytes().await.ok()?.to_vec();
    debug!("fetched {:?} artwork ({} bytes)", kind, bytes.len());
    Some(bytes)
}

/// Fetch an image Drop already holds, used when SteamGridDB has nothing (or no
/// key is configured). These are the same objects the library renders, so a
/// game always ends up with *something* rather than a blank tile.
pub async fn fetch_drop_object(object_id: &str) -> Result<Vec<u8>, SteamCommandError> {
    use database::DB;
    use remote::{auth::generate_authorization_header, utils::DROP_CLIENT_ASYNC};

    let url = format!("{}api/v1/client/object/{object_id}", DB.fetch_base_url());
    let response = DROP_CLIENT_ASYNC
        .get(url)
        .header("Authorization", generate_authorization_header())
        .send()
        .await
        .map_err(|e| SteamCommandError::Artwork(e.to_string()))?;

    if !response.status().is_success() {
        return Err(SteamCommandError::Artwork(format!(
            "Drop returned {} for object {object_id}",
            response.status()
        )));
    }

    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| SteamCommandError::Artwork(e.to_string()))
}
