#![feature(nonpoison_mutex)]
#![feature(sync_nonpoison)]

//! ZOUGCLOUD(ZC-008): local game playtime.
//!
//! Drop's own counter, owned entirely by the client. It does not depend on
//! Steam, on GOG, or on the Drop server, and it is stored outside `drop.db` so
//! that an upstream schema change can never cost a member their hours — and so
//! that rebasing onto a new Drop release does not touch it.
//!
//! The one invariant: **at most one active session per game**, with an owner.
//! Drop's process manager and the external process watcher can both report on
//! the same game without ever counting it twice.

use std::{
    path::PathBuf,
    sync::{OnceLock, nonpoison::Mutex},
};

pub mod format;
pub mod model;
pub mod store;
pub mod tracker;

pub use format::{format_last_played, format_playtime};
pub use model::{GamePlaytime, SessionOwner};
pub use tracker::PlaytimeTracker;

/// Process-wide tracker, mirroring how upstream exposes `PROCESS_MANAGER`.
pub static PLAYTIME: PlaytimeWrapper = PlaytimeWrapper::new();

pub struct PlaytimeWrapper(OnceLock<Mutex<PlaytimeTracker>>);

impl PlaytimeWrapper {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// Called once at startup with the ZougCloud data directory. Recovers any
    /// session left open by a crash.
    pub fn init(path: PathBuf) {
        let _ = PLAYTIME
            .0
            .set(Mutex::new(PlaytimeTracker::load(path, now())));
    }

    pub fn lock(&self) -> impl std::ops::DerefMut<Target = PlaytimeTracker> + '_ {
        self.0
            .get()
            .expect("playtime tracker used before init")
            .lock()
    }

    /// True once `init` has run. Lets hooks in upstream code no-op cleanly if
    /// they somehow fire early, rather than panicking.
    pub fn is_ready(&self) -> bool {
        self.0.get().is_some()
    }
}

/// Current unix time. Every tracker method takes the time explicitly so the
/// tests stay deterministic; this is the single production source.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
