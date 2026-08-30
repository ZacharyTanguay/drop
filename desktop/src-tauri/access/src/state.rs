//! ZOUGCLOUD(ZC-011): process-wide access state.
//!
//! Two very different callers need the same answer: the Tauri commands that
//! filter what the Desktop shows, and the `server://` proxy in the `remote`
//! crate that guards Store writes. Holding the manifest here — rather than
//! passing it around — is what keeps those two from drifting apart, which is
//! the failure that would let a hidden game be added anyway.

use std::{
    path::PathBuf,
    sync::{OnceLock, nonpoison::Mutex},
};

use crate::{
    model::{AccessDecision, AccessManifest, Viewer},
    store::{ApplyOutcome, ManifestCache, ManifestResponse},
};

pub static ACCESS: AccessState = AccessState::new();

struct Inner {
    cache: ManifestCache,
    /// `None` until the user is known (before sign-in, or offline with no
    /// cached user).
    viewer: Option<Viewer>,
}

pub struct AccessState(OnceLock<Mutex<Inner>>);

impl AccessState {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    pub fn init(path: PathBuf) {
        let _ = ACCESS.0.set(Mutex::new(Inner {
            cache: ManifestCache::load(path),
            viewer: None,
        }));
    }

    pub fn is_ready(&self) -> bool {
        self.0.get().is_some()
    }

    pub fn set_viewer(&self, viewer: Option<Viewer>) {
        let Some(inner) = self.0.get() else { return };
        inner.lock().viewer = viewer;
    }

    pub fn viewer_is_admin(&self) -> bool {
        self.0
            .get()
            .and_then(|i| i.lock().viewer.as_ref().map(Viewer::is_admin))
            .unwrap_or(false)
    }

    pub fn revision(&self) -> Option<u64> {
        self.0.get()?.lock().cache.revision()
    }

    pub fn etag(&self) -> Option<String> {
        self.0.get()?.lock().cache.etag().map(str::to_owned)
    }

    /// Store a freshly fetched manifest. Returns whether it was applied.
    pub fn store(&self, manifest: AccessManifest, etag: Option<String>, now: i64) -> bool {
        let Some(inner) = self.0.get() else {
            return false;
        };
        inner
            .lock()
            .cache
            .store(manifest, etag, now)
            .unwrap_or(false)
    }

    pub fn touch(&self, now: i64) {
        let Some(inner) = self.0.get() else { return };
        inner.lock().cache.touch(now);
    }

    /// Fold one poll result into the cache. All the interesting behaviour —
    /// notably keeping the last known-good manifest when the remote misbehaves
    /// — lives in [`crate::store::apply_response`], which is tested without a
    /// network.
    pub fn apply(&self, response: ManifestResponse, now: i64) -> ApplyOutcome {
        let Some(inner) = self.0.get() else {
            return ApplyOutcome::NoPolicy;
        };
        crate::store::apply_response(&mut inner.lock().cache, response, now)
    }

    /// The decision for one game.
    ///
    /// Fails closed in every degenerate case — state not ready, no user known,
    /// no manifest fetched yet. None of those may widen access; the worst
    /// outcome is a member briefly seeing less than they should, never more.
    pub fn decide(&self, game_id: &str) -> AccessDecision {
        let Some(inner) = self.0.get() else {
            return AccessDecision::DeniedUnknownPolicy;
        };
        let guard = inner.lock();

        let Some(viewer) = guard.viewer.as_ref() else {
            return AccessDecision::DeniedUnknownPolicy;
        };

        // Checked before the manifest so the admin is never locked out by a
        // missing or unfetched one.
        if viewer.is_admin() {
            return AccessDecision::AllowedAsAdmin;
        }

        let Some(manifest) = guard.cache.manifest() else {
            return AccessDecision::DeniedUnknownPolicy;
        };

        crate::decide(manifest, viewer, game_id)
    }

    pub fn is_accessible(&self, game_id: &str) -> bool {
        self.decide(game_id).is_allowed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ADMIN_USERNAME;

    // ACCESS is process-wide, so these exercise the degenerate paths that do
    // not need init -- the ones that must fail closed.

    #[test]
    fn an_uninitialised_state_denies_everything() {
        let fresh = AccessState::new();
        assert_eq!(
            fresh.decide("any-game"),
            AccessDecision::DeniedUnknownPolicy
        );
        assert!(!fresh.is_accessible("any-game"));
        assert!(!fresh.viewer_is_admin());
        assert!(fresh.revision().is_none());
    }

    #[test]
    fn the_admin_is_allowed_even_with_no_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = AccessState::new();
        let _ = state.0.set(Mutex::new(Inner {
            cache: ManifestCache::load(dir.path().join("v.json")),
            viewer: Some(Viewer {
                user_id: "a".to_owned(),
                username: ADMIN_USERNAME.to_owned(),
            }),
        }));

        // No manifest has ever been fetched: a member would be denied, but the
        // admin must never be locked out of their own client.
        assert_eq!(state.decide("g"), AccessDecision::AllowedAsAdmin);
    }

    #[test]
    fn a_member_with_no_manifest_is_denied_not_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = AccessState::new();
        let _ = state.0.set(Mutex::new(Inner {
            cache: ManifestCache::load(dir.path().join("v.json")),
            viewer: Some(Viewer {
                user_id: "b".to_owned(),
                username: "bob".to_owned(),
            }),
        }));

        assert_eq!(state.decide("g"), AccessDecision::DeniedUnknownPolicy);
    }
}
