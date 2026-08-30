use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
};

use log::{error, warn};
use serde::{Deserialize, Serialize};

use crate::model::{AccessManifest, SCHEMA_VERSION};

/// The cached manifest, with enough context to decide whether a refetch is
/// needed and to reason about staleness.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CachedManifest {
    pub schema_version: u32,
    pub revision: u64,
    /// Unix seconds of the last successful fetch.
    pub fetched_at: i64,
    /// Whatever the remote returned for `If-None-Match`, so a poll that finds
    /// nothing new costs one 304 rather than a full download.
    #[serde(default)]
    pub etag: Option<String>,
    pub data: AccessManifest,
}

/// Local persistence for the access manifest.
///
/// The critical property is what happens when the remote is unreachable: the
/// last known-good manifest keeps applying. **Never** falling back to
/// "everything visible", which would quietly undo every restriction the moment
/// GitHub had an outage.
pub struct ManifestCache {
    path: PathBuf,
    cached: Option<CachedManifest>,
}

impl ManifestCache {
    pub fn load(path: PathBuf) -> Self {
        let cached = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<CachedManifest>(&text) {
                Ok(c) => Some(c),
                Err(e) => {
                    // Not fatal, and deliberately not "allow everything":
                    // callers treat a missing cache as "nothing granted yet".
                    error!("cached access manifest at {} is unreadable: {e}", path.display());
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                error!("could not read {}: {e}", path.display());
                None
            }
        };

        Self { path, cached }
    }

    /// The manifest in force. `None` before the first successful fetch —
    /// callers must treat that as "no grants", not as "no restrictions".
    pub fn manifest(&self) -> Option<&AccessManifest> {
        self.cached.as_ref().map(|c| &c.data)
    }

    pub fn revision(&self) -> Option<u64> {
        self.cached.as_ref().map(|c| c.revision)
    }

    pub fn etag(&self) -> Option<&str> {
        self.cached.as_ref()?.etag.as_deref()
    }

    pub fn fetched_at(&self) -> Option<i64> {
        self.cached.as_ref().map(|c| c.fetched_at)
    }

    /// Replace the cache after a successful fetch.
    ///
    /// Only a manifest that is **valid and not older** replaces what is held.
    /// Everything else leaves the last known-good copy in force, because the
    /// alternative — dropping to an empty policy — would make a member's whole
    /// library vanish because a remote had a bad day.
    pub fn store(
        &mut self,
        manifest: AccessManifest,
        etag: Option<String>,
        now: i64,
    ) -> std::io::Result<bool> {
        // A manifest from a future schema cannot be interpreted safely: fields
        // this build ignores might be the ones restricting access. Keep what we
        // have rather than applying half of it.
        if manifest.schema_version > SCHEMA_VERSION {
            warn!(
                "ignoring access manifest with schema v{} (this build understands v{SCHEMA_VERSION}); \
                 keeping the cached copy",
                manifest.schema_version
            );
            return Ok(false);
        }

        if let Some(current) = &self.cached
            && manifest.revision < current.revision
        {
            warn!(
                "ignoring access manifest revision {} older than the cached {}",
                manifest.revision, current.revision
            );
            return Ok(false);
        }

        let cached = CachedManifest {
            schema_version: manifest.schema_version,
            revision: manifest.revision,
            fetched_at: now,
            etag,
            data: manifest,
        };

        self.write(&cached)?;
        self.cached = Some(cached);
        Ok(true)
    }

    /// Record that a poll confirmed the cache is current, without rewriting
    /// the whole manifest.
    pub fn touch(&mut self, now: i64) {
        let Some(cached) = self.cached.as_mut() else {
            return;
        };
        cached.fetched_at = now;
        let snapshot = cached.clone();
        if let Err(e) = self.write(&snapshot) {
            warn!("could not update the access manifest timestamp: {e}");
        }
    }

    /// Temp file, flush, atomic rename — the same discipline as playtime. A
    /// half-written manifest would parse as nothing and drop every grant.
    fn write(&self, cached: &CachedManifest) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(cached)
            .map_err(|e| std::io::Error::other(format!("could not serialise manifest: {e}")))?;

        let temp = self.path.with_extension("json.tmp");
        {
            let mut file = File::create(&temp)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&temp, &self.path)
    }
}

/// What one poll of the remote produced.
///
/// Kept separate from any HTTP client so the decision logic below can be
/// tested exhaustively without a network — these are exactly the cases that
/// decide whether a member keeps their library during an outage.
#[derive(Debug)]
pub enum ManifestResponse {
    /// The remote confirmed our ETag is current (HTTP 304).
    NotModified,
    /// A body arrived. It may still be unparseable.
    Body {
        text: String,
        etag: Option<String>,
    },
    /// No usable answer: network error, timeout, non-2xx.
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// A newer valid manifest was applied.
    Updated { revision: u64 },
    /// The remote confirmed nothing changed.
    Unchanged,
    /// The response was unusable; the last known-good manifest still applies.
    RetainedCache,
    /// Nothing usable and nothing cached. Members are denied until a manifest
    /// arrives — free and all cannot be assumed without an authoritative copy.
    NoPolicy,
}

/// Fold one poll result into the cache.
///
/// The rule that matters: **a remote failure never erases a valid cached
/// policy.** Only a valid, non-older manifest replaces what is held. A member
/// whose library worked yesterday must not lose it because GitHub had an
/// outage or served something malformed.
pub fn apply_response(
    cache: &mut ManifestCache,
    response: ManifestResponse,
    now: i64,
) -> ApplyOutcome {
    let had_cache = cache.manifest().is_some();

    match response {
        ManifestResponse::NotModified => {
            cache.touch(now);
            if had_cache {
                ApplyOutcome::Unchanged
            } else {
                // A 304 with nothing cached should not happen (we would not
                // have sent If-None-Match), but it must not be read as "empty".
                ApplyOutcome::NoPolicy
            }
        }

        ManifestResponse::Body { text, etag } => {
            match serde_json::from_str::<AccessManifest>(&text) {
                Ok(manifest) => match cache.store(manifest, etag, now) {
                    Ok(true) => ApplyOutcome::Updated {
                        revision: cache.revision().unwrap_or(0),
                    },
                    // Rejected as older or from an unsupported schema.
                    Ok(false) => retained_or_none(had_cache),
                    Err(e) => {
                        error!("could not persist the access manifest: {e}");
                        retained_or_none(had_cache)
                    }
                },
                Err(e) => {
                    warn!("remote access manifest is malformed, keeping the cached copy: {e}");
                    retained_or_none(had_cache)
                }
            }
        }

        ManifestResponse::Unavailable => retained_or_none(had_cache),
    }
}

fn retained_or_none(had_cache: bool) -> ApplyOutcome {
    if had_cache {
        ApplyOutcome::RetainedCache
    } else {
        ApplyOutcome::NoPolicy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccessMode, GamePolicy};

    fn manifest(revision: u64) -> AccessManifest {
        let mut m = AccessManifest {
            revision,
            ..Default::default()
        };
        m.games.insert(
            "g".to_owned(),
            GamePolicy {
                access_mode: Some(AccessMode::Free),
                price: None,
            },
        );
        m
    }

    #[test]
    fn a_fresh_install_has_no_manifest_and_therefore_no_grants() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ManifestCache::load(dir.path().join("visibility.json"));
        assert!(cache.manifest().is_none());
        assert!(cache.revision().is_none());
    }

    #[test]
    fn storing_then_reloading_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("visibility.json");

        let mut cache = ManifestCache::load(path.clone());
        assert!(cache.store(manifest(7), Some("etag-7".to_owned()), 100).expect("store"));

        let reloaded = ManifestCache::load(path);
        assert_eq!(reloaded.revision(), Some(7));
        assert_eq!(reloaded.etag(), Some("etag-7"));
        assert_eq!(reloaded.fetched_at(), Some(100));
        assert!(reloaded.manifest().expect("data").games.contains_key("g"));
    }

    #[test]
    fn the_cache_survives_the_remote_being_unreachable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("visibility.json");

        let mut cache = ManifestCache::load(path.clone());
        cache.store(manifest(3), None, 100).expect("store");

        // A later run with no network simply reloads what it had. Nothing in
        // this path can widen access.
        let offline = ManifestCache::load(path);
        assert_eq!(offline.revision(), Some(3));
    }

    #[test]
    fn an_older_revision_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cache = ManifestCache::load(dir.path().join("visibility.json"));

        cache.store(manifest(10), None, 100).expect("store");
        let accepted = cache.store(manifest(9), None, 200).expect("store older");

        assert!(!accepted, "a stale copy must not undo a newer one");
        assert_eq!(cache.revision(), Some(10));
    }

    #[test]
    fn the_same_revision_is_accepted_so_edits_can_be_republished() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cache = ManifestCache::load(dir.path().join("visibility.json"));
        cache.store(manifest(5), None, 100).expect("store");
        assert!(cache.store(manifest(5), None, 200).expect("restore"));
    }

    #[test]
    fn writes_are_atomic_and_leave_no_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("visibility.json");

        let mut cache = ManifestCache::load(path.clone());
        cache.store(manifest(1), None, 1).expect("store");

        assert!(path.is_file());
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn an_unreadable_cache_yields_no_grants_rather_than_all_of_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("visibility.json");
        fs::write(&path, b"{ not json").expect("write");

        let cache = ManifestCache::load(path);
        // The failure direction that matters: nothing is granted.
        assert!(cache.manifest().is_none());
    }
}

// ZOUGCLOUD(ZC-011): the last-known-good contract.
//
// The distinction that matters, and that these tests pin down:
//   - never fetched successfully  -> fail closed
//   - fetched before, remote now unavailable -> KEEP APPLYING the cached copy
//
// Collapsing the second case into the first would make a member's whole
// library disappear during a GitHub outage.
#[cfg(test)]
mod last_known_good {
    use super::*;
    use crate::model::{AccessMode, Viewer};

    fn body(revision: u64, mode: &str) -> String {
        format!(
            r#"{{"schemaVersion":1,"revision":{revision},
               "games":{{"g":{{"accessMode":"{mode}"}}}},"users":{{}}}}"#
        )
    }

    fn cache(dir: &tempfile::TempDir) -> ManifestCache {
        ManifestCache::load(dir.path().join("visibility.json"))
    }

    fn seed(c: &mut ManifestCache, revision: u64) {
        let outcome = apply_response(
            c,
            ManifestResponse::Body {
                text: body(revision, "free"),
                etag: Some(format!("etag-{revision}")),
            },
            100,
        );
        assert_eq!(outcome, ApplyOutcome::Updated { revision });
    }

    #[test]
    fn first_launch_with_no_cache_and_no_remote_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);

        assert_eq!(
            apply_response(&mut c, ManifestResponse::Unavailable, 1),
            ApplyOutcome::NoPolicy
        );
        assert!(c.manifest().is_none());
    }

    #[test]
    fn a_cached_manifest_survives_the_remote_going_away() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);
        seed(&mut c, 42);

        assert_eq!(
            apply_response(&mut c, ManifestResponse::Unavailable, 200),
            ApplyOutcome::RetainedCache
        );
        assert_eq!(c.revision(), Some(42), "revision 42 must keep applying");
        assert!(c.manifest().expect("data").games.contains_key("g"));
    }

    #[test]
    fn a_304_keeps_the_cached_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);
        seed(&mut c, 42);

        assert_eq!(
            apply_response(&mut c, ManifestResponse::NotModified, 300),
            ApplyOutcome::Unchanged
        );
        assert_eq!(c.revision(), Some(42));
        assert_eq!(c.fetched_at(), Some(300), "the poll timestamp advances");
    }

    #[test]
    fn a_malformed_response_never_overwrites_a_valid_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);
        seed(&mut c, 42);

        let outcome = apply_response(
            &mut c,
            ManifestResponse::Body {
                text: "<html>502 Bad Gateway</html>".to_owned(),
                etag: Some("garbage".to_owned()),
            },
            400,
        );

        assert_eq!(outcome, ApplyOutcome::RetainedCache);
        assert_eq!(c.revision(), Some(42));
        assert_eq!(c.etag(), Some("etag-42"), "the good ETag is kept too");
    }

    #[test]
    fn a_manifest_from_a_newer_schema_is_refused_and_the_cache_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);
        seed(&mut c, 42);

        let outcome = apply_response(
            &mut c,
            ManifestResponse::Body {
                text: r#"{"schemaVersion":99,"revision":43,"games":{},"users":{}}"#.to_owned(),
                etag: None,
            },
            500,
        );

        // Applying only the parts we understand could drop the very fields
        // that restrict access.
        assert_eq!(outcome, ApplyOutcome::RetainedCache);
        assert_eq!(c.revision(), Some(42));
    }

    #[test]
    fn a_newer_valid_revision_replaces_the_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);
        seed(&mut c, 42);

        let outcome = apply_response(
            &mut c,
            ManifestResponse::Body {
                text: body(43, "gated"),
                etag: Some("etag-43".to_owned()),
            },
            600,
        );

        assert_eq!(outcome, ApplyOutcome::Updated { revision: 43 });
        assert_eq!(c.revision(), Some(43));
        assert_eq!(c.etag(), Some("etag-43"));
        assert_eq!(
            c.manifest().expect("data").games["g"].access_mode,
            Some(AccessMode::Gated),
            "the new policy is in force"
        );
    }

    #[test]
    fn a_corrupt_local_cache_with_no_remote_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("visibility.json");
        fs::write(&path, b"{ truncated").expect("write");

        let mut c = ManifestCache::load(path);
        assert_eq!(
            apply_response(&mut c, ManifestResponse::Unavailable, 1),
            ApplyOutcome::NoPolicy
        );
        assert!(c.manifest().is_none());
    }

    #[test]
    fn a_corrupt_local_cache_recovers_from_the_next_good_fetch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("visibility.json");
        fs::write(&path, b"{ truncated").expect("write");

        let mut c = ManifestCache::load(path);
        // No cached revision to compare against, so any valid manifest applies.
        assert_eq!(
            apply_response(
                &mut c,
                ManifestResponse::Body {
                    text: body(7, "free"),
                    etag: None
                },
                10
            ),
            ApplyOutcome::Updated { revision: 7 }
        );
    }

    #[test]
    fn the_admin_is_unaffected_by_manifest_availability() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);
        apply_response(&mut c, ManifestResponse::Unavailable, 1);

        let admin = Viewer {
            user_id: "a".to_owned(),
            username: crate::ADMIN_USERNAME.to_owned(),
        };
        // No manifest at all, and the admin still sees everything.
        let empty = AccessManifest::default();
        assert!(crate::is_game_accessible(&empty, &admin, "anything"));
    }

    #[test]
    fn an_unknown_member_gets_free_and_all_but_not_gated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = cache(&dir);

        let text = r#"{"schemaVersion":2,"revision":1,"games":{
            "free-game":{"accessMode":"free"},
            "gated-game":{"accessMode":"gated"}},
            "users":{"all-member":{"accessMode":"all","allowedGames":[]}}}"#;
        apply_response(
            &mut c,
            ManifestResponse::Body {
                text: text.to_owned(),
                etag: None,
            },
            1,
        );

        let m = c.manifest().expect("data");

        // Someone who has never been configured is Custom by default.
        let stranger = Viewer {
            user_id: "never-seen-before".to_owned(),
            username: "newcomer".to_owned(),
        };
        assert!(crate::is_game_accessible(m, &stranger, "free-game"));
        assert!(!crate::is_game_accessible(m, &stranger, "gated-game"));
        assert!(!crate::is_game_accessible(m, &stranger, "unlisted"));

        // An All member gets everything, including a game with no policy.
        let everything = Viewer {
            user_id: "all-member".to_owned(),
            username: "bob".to_owned(),
        };
        assert!(crate::is_game_accessible(m, &everything, "free-game"));
        assert!(crate::is_game_accessible(m, &everything, "gated-game"));
        assert!(crate::is_game_accessible(m, &everything, "unlisted"));
    }
}
