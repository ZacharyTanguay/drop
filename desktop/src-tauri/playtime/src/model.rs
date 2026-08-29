use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Bump when the on-disk shape changes, and add a migration in
/// `store::migrate`. Never reuse a number.
pub const SCHEMA_VERSION: u32 = 1;

/// Who opened a session. Only the owner may close it, which is what stops the
/// external watcher from ending a session Drop is managing (and vice versa).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SessionOwner {
    /// Drop launched the game itself and knows its real lifecycle.
    Drop,
    /// The background watcher saw the executable appear (e.g. launched from
    /// Steam).
    Watcher,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GamePlaytime {
    pub total_playtime_seconds: u64,
    /// Unix seconds. `None` until the game has been played once.
    pub last_played_at: Option<i64>,
}

/// A session in progress. Persisted so a crash cannot lose it silently.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSession {
    pub owner: SessionOwner,
    pub started_at: i64,
    /// Refreshed while the game is seen running. On recovery this — not the
    /// current time — is what the session is closed at.
    pub heartbeat_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlaytimeState {
    pub schema_version: u32,
    #[serde(default)]
    pub games: HashMap<String, GamePlaytime>,
    /// Sessions that were running when the file was last written.
    #[serde(default)]
    pub active: HashMap<String, ActiveSession>,
}

impl Default for PlaytimeState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            games: HashMap::new(),
            active: HashMap::new(),
        }
    }
}
