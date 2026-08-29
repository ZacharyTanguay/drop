use std::path::PathBuf;

use log::{debug, info, warn};

use crate::{
    model::{ActiveSession, GamePlaytime, SessionOwner},
    store::PlaytimeStore,
};

/// Tracks how long each game has been played, locally.
///
/// The invariant everything else rests on: **at most one active session per
/// game**. `begin_session` is a no-op when one is already open and
/// `end_session` only closes a session belonging to the same owner. That is
/// what makes it safe for Drop's process manager and the external watcher to
/// both report on the same game without ever counting it twice.
pub struct PlaytimeTracker {
    store: PlaytimeStore,
}

impl PlaytimeTracker {
    /// Load, recovering any session left open by a crash.
    pub fn load(path: PathBuf, now: i64) -> Self {
        let mut tracker = Self {
            store: PlaytimeStore::load(path),
        };
        tracker.recover_orphan_sessions(now);
        tracker
    }

    /// Close sessions that were still open when Drop last stopped.
    ///
    /// Closed at the **last heartbeat**, never at the current time. A machine
    /// can sit powered off for a day between the crash and the next launch;
    /// crediting that gap would invent hours the member never played.
    fn recover_orphan_sessions(&mut self, now: i64) {
        let orphans: Vec<(String, ActiveSession)> = self
            .store
            .state()
            .active
            .iter()
            .map(|(id, session)| (id.clone(), session.clone()))
            .collect();

        if orphans.is_empty() {
            return;
        }

        for (game_id, session) in orphans {
            let seconds = session.heartbeat_at.saturating_sub(session.started_at).max(0) as u64;
            warn!(
                "recovering orphaned session for {game_id}: crediting {seconds}s up to the last \
                 heartbeat (not up to now, which would invent {}s)",
                now.saturating_sub(session.started_at).max(0)
            );
            self.credit(&game_id, seconds, session.heartbeat_at);
            self.store.state_mut().active.remove(&game_id);
        }

        self.persist();
    }

    fn credit(&mut self, game_id: &str, seconds: u64, played_at: i64) {
        let entry = self
            .store
            .state_mut()
            .games
            .entry(game_id.to_owned())
            .or_default();
        entry.total_playtime_seconds = entry.total_playtime_seconds.saturating_add(seconds);
        // Keep the newest timestamp: a recovered session may be older than a
        // session that has already been recorded.
        if entry.last_played_at.is_none_or(|existing| played_at > existing) {
            entry.last_played_at = Some(played_at);
        }
    }

    fn persist(&self) {
        if let Err(e) = self.store.save() {
            warn!("could not save playtime: {e}");
        }
    }

    /// Open a session. Returns false when one is already open — that is the
    /// normal case for the watcher noticing a game Drop launched itself, not an
    /// error.
    pub fn begin_session(&mut self, game_id: &str, owner: SessionOwner, now: i64) -> bool {
        if let Some(existing) = self.store.state().active.get(game_id) {
            debug!(
                "not starting a {owner:?} session for {game_id}: a {:?} session is already open",
                existing.owner
            );
            return false;
        }

        self.store.state_mut().active.insert(
            game_id.to_owned(),
            ActiveSession {
                owner,
                started_at: now,
                heartbeat_at: now,
            },
        );
        info!("playtime session started for {game_id} ({owner:?})");
        self.persist();
        true
    }

    /// Close a session and credit its duration. Returns the seconds credited.
    pub fn end_session(&mut self, game_id: &str, owner: SessionOwner, now: i64) -> Option<u64> {
        let session = self.store.state().active.get(game_id).cloned()?;

        if session.owner != owner {
            debug!(
                "{owner:?} tried to end {game_id}'s session but it belongs to {:?}",
                session.owner
            );
            return None;
        }

        // A clock that moved backwards must not subtract playtime.
        let seconds = now.saturating_sub(session.started_at).max(0) as u64;
        self.credit(game_id, seconds, now);
        self.store.state_mut().active.remove(game_id);
        info!("playtime session ended for {game_id} ({owner:?}): +{seconds}s");
        self.persist();
        Some(seconds)
    }

    /// Mark a session as still alive. This is what bounds the damage of a
    /// crash to one heartbeat interval.
    pub fn heartbeat(&mut self, game_id: &str, now: i64) {
        let Some(session) = self.store.state_mut().active.get_mut(game_id) else {
            return;
        };
        session.heartbeat_at = now;
        self.persist();
    }

    pub fn active_owner(&self, game_id: &str) -> Option<SessionOwner> {
        self.store.state().active.get(game_id).map(|s| s.owner)
    }

    pub fn is_active(&self, game_id: &str) -> bool {
        self.store.state().active.contains_key(game_id)
    }

    /// Recorded playtime, excluding any session currently in progress. Games
    /// that have never been played report zero rather than being absent, so
    /// callers do not have to special-case them.
    pub fn playtime(&self, game_id: &str) -> GamePlaytime {
        self.store
            .state()
            .games
            .get(game_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Every game with recorded playtime.
    pub fn all(&self) -> Vec<(String, GamePlaytime)> {
        self.store
            .state()
            .games
            .iter()
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect()
    }

    /// Games with a session open right now.
    pub fn active_games(&self) -> Vec<String> {
        self.store.state().active.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SCHEMA_VERSION;

    fn tracker(dir: &tempfile::TempDir) -> PlaytimeTracker {
        PlaytimeTracker::load(dir.path().join("playtime.json"), 1_000)
    }

    #[test]
    fn a_new_game_has_no_playtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let t = tracker(&dir);
        let p = t.playtime("game");
        assert_eq!(p.total_playtime_seconds, 0);
        assert_eq!(p.last_played_at, None);
    }

    #[test]
    fn a_single_session_is_credited() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);

        assert!(t.begin_session("game", SessionOwner::Drop, 1_000));
        assert_eq!(t.end_session("game", SessionOwner::Drop, 1_120), Some(120));

        let p = t.playtime("game");
        assert_eq!(p.total_playtime_seconds, 120);
        assert_eq!(p.last_played_at, Some(1_120));
    }

    #[test]
    fn two_sequential_sessions_add_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);

        t.begin_session("game", SessionOwner::Drop, 1_000);
        t.end_session("game", SessionOwner::Drop, 1_060);
        t.begin_session("game", SessionOwner::Watcher, 2_000);
        t.end_session("game", SessionOwner::Watcher, 2_030);

        assert_eq!(t.playtime("game").total_playtime_seconds, 90);
        assert_eq!(t.playtime("game").last_played_at, Some(2_030));
    }

    #[test]
    fn a_launch_that_never_started_credits_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);

        // The caller only calls begin_session after a successful spawn, so a
        // failed launch leaves no session and no time.
        assert!(!t.is_active("game"));
        assert_eq!(t.end_session("game", SessionOwner::Drop, 2_000), None);
        assert_eq!(t.playtime("game").total_playtime_seconds, 0);
    }

    #[test]
    fn the_watcher_cannot_double_count_a_drop_launch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);

        t.begin_session("game", SessionOwner::Drop, 1_000);
        // The watcher sees the same executable two polls later.
        assert!(!t.begin_session("game", SessionOwner::Watcher, 1_007));
        assert!(!t.begin_session("game", SessionOwner::Watcher, 1_014));

        // And it must not be able to close a session it does not own.
        assert_eq!(t.end_session("game", SessionOwner::Watcher, 1_020), None);
        assert!(t.is_active("game"));

        assert_eq!(t.end_session("game", SessionOwner::Drop, 1_100), Some(100));
        assert_eq!(t.playtime("game").total_playtime_seconds, 100);
    }

    #[test]
    fn a_watcher_only_launch_is_tracked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);

        // Launched from Steam: Drop never saw a spawn.
        assert!(t.begin_session("game", SessionOwner::Watcher, 500));
        assert_eq!(t.active_owner("game"), Some(SessionOwner::Watcher));
        assert_eq!(t.end_session("game", SessionOwner::Watcher, 800), Some(300));
        assert_eq!(t.playtime("game").total_playtime_seconds, 300);
    }

    #[test]
    fn an_active_session_survives_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("playtime.json");

        let mut t = PlaytimeTracker::load(path.clone(), 1_000);
        t.begin_session("game", SessionOwner::Drop, 1_000);
        t.heartbeat("game", 1_300);
        drop(t);

        // Reloading recovers it rather than losing it.
        let t = PlaytimeTracker::load(path, 9_999);
        assert!(!t.is_active("game"));
        assert_eq!(t.playtime("game").total_playtime_seconds, 300);
    }

    #[test]
    fn orphan_recovery_stops_at_the_last_heartbeat() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("playtime.json");

        // Started 20:00, last seen 20:42, then the machine died.
        let mut t = PlaytimeTracker::load(path.clone(), 0);
        t.begin_session("game", SessionOwner::Drop, 72_000);
        t.heartbeat("game", 74_520);
        drop(t);

        // Drop reopens the next morning, twelve hours later.
        let t = PlaytimeTracker::load(path, 115_200);

        // 42 minutes, not 12 hours.
        assert_eq!(t.playtime("game").total_playtime_seconds, 2_520);
        assert_eq!(t.playtime("game").last_played_at, Some(74_520));
    }

    #[test]
    fn a_session_with_no_heartbeat_credits_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("playtime.json");

        // Crashed immediately after starting: heartbeat == started_at.
        let mut t = PlaytimeTracker::load(path.clone(), 1_000);
        t.begin_session("game", SessionOwner::Drop, 1_000);
        drop(t);

        let t = PlaytimeTracker::load(path, 500_000);
        assert_eq!(t.playtime("game").total_playtime_seconds, 0);
    }

    #[test]
    fn totals_persist_across_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("playtime.json");

        let mut t = PlaytimeTracker::load(path.clone(), 0);
        t.begin_session("game", SessionOwner::Drop, 100);
        t.end_session("game", SessionOwner::Drop, 700);
        drop(t);

        let t = PlaytimeTracker::load(path, 1_000);
        assert_eq!(t.playtime("game").total_playtime_seconds, 600);
    }

    #[test]
    fn the_file_is_written_atomically_and_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("playtime.json");

        let mut t = PlaytimeTracker::load(path.clone(), 0);
        t.begin_session("game", SessionOwner::Drop, 100);
        t.end_session("game", SessionOwner::Drop, 200);

        assert!(path.is_file());
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the temp file must be renamed away, not left beside the real one"
        );

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"schemaVersion\": 1"));
    }

    #[test]
    fn a_corrupt_file_is_preserved_not_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("playtime.json");
        std::fs::write(&path, b"{ this is not json").expect("write");

        let mut t = PlaytimeTracker::load(path.clone(), 0);
        t.begin_session("game", SessionOwner::Drop, 100);
        t.end_session("game", SessionOwner::Drop, 160);

        // A fresh, valid file exists...
        assert_eq!(t.playtime("game").total_playtime_seconds, 60);

        // ...and the unreadable one was kept for inspection.
        let preserved: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("playtime.corrupt-")
            })
            .collect();
        assert_eq!(preserved.len(), 1, "the corrupt file must not be destroyed");
        assert_eq!(
            std::fs::read_to_string(preserved[0].path()).expect("read"),
            "{ this is not json"
        );
    }

    #[test]
    fn a_clock_that_moves_backwards_never_subtracts_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);

        t.begin_session("game", SessionOwner::Drop, 5_000);
        assert_eq!(t.end_session("game", SessionOwner::Drop, 4_000), Some(0));
        assert_eq!(t.playtime("game").total_playtime_seconds, 0);
    }

    #[test]
    fn sessions_for_different_games_are_independent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);

        t.begin_session("a", SessionOwner::Drop, 1_000);
        t.begin_session("b", SessionOwner::Watcher, 1_000);
        assert_eq!(t.active_games().len(), 2);

        t.end_session("a", SessionOwner::Drop, 1_100);
        assert_eq!(t.playtime("a").total_playtime_seconds, 100);
        assert_eq!(t.playtime("b").total_playtime_seconds, 0);
        assert!(t.is_active("b"));
    }

    #[test]
    fn the_schema_version_is_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut t = tracker(&dir);
        t.begin_session("game", SessionOwner::Drop, 1);
        t.end_session("game", SessionOwner::Drop, 2);

        let text = std::fs::read_to_string(t.store.path()).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("parse");
        assert_eq!(parsed["schemaVersion"], SCHEMA_VERSION);
    }
}
