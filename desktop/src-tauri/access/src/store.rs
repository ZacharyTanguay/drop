use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
};

use log::{error, warn};
use serde::{Deserialize, Serialize};

use crate::model::AccessManifest;

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
    /// Refuses to go backwards: an older revision arriving out of order (a
    /// stale CDN copy, say) must not undo a newer one already applied.
    pub fn store(
        &mut self,
        manifest: AccessManifest,
        etag: Option<String>,
        now: i64,
    ) -> std::io::Result<bool> {
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
