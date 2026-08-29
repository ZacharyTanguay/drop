use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use log::{error, info, warn};

use crate::model::{PlaytimeState, SCHEMA_VERSION};

/// Reads and writes the playtime file.
///
/// Playtime is user data that exists nowhere else — not on the Drop server, not
/// in Steam — so losing it is unrecoverable. Every write therefore goes through
/// a temporary file and an atomic rename, and a file we cannot parse is
/// preserved rather than replaced.
pub struct PlaytimeStore {
    path: PathBuf,
    state: PlaytimeState,
}

impl PlaytimeStore {
    /// Load the state, recovering rather than failing.
    ///
    /// A missing file is a new install. An unreadable or unparseable one is
    /// moved aside and reported: silently starting from zero would quietly
    /// destroy someone's history, and the file may still be salvageable by
    /// hand.
    pub fn load(path: PathBuf) -> Self {
        let state = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<PlaytimeState>(&text) {
                Ok(state) => Some(migrate(state)),
                Err(e) => {
                    error!("playtime file at {} is corrupt: {e}", path.display());
                    preserve_corrupt(&path);
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                error!("could not read playtime file {}: {e}", path.display());
                preserve_corrupt(&path);
                None
            }
        };

        Self {
            path,
            state: state.unwrap_or_default(),
        }
    }

    pub fn state(&self) -> &PlaytimeState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut PlaytimeState {
        &mut self.state
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persist atomically: write a sibling temp file, flush it to disk, then
    /// rename over the target. A crash can leave the temp file behind but can
    /// never leave a half-written playtime file.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&self.state)
            .map_err(|e| std::io::Error::other(format!("could not serialise playtime: {e}")))?;

        let temp = self.path.with_extension("json.tmp");
        {
            let mut file = File::create(&temp)?;
            file.write_all(json.as_bytes())?;
            // Without this the rename can land before the bytes do, which on a
            // power loss leaves an empty file where the history used to be.
            file.sync_all()?;
        }

        // On Windows this maps to MoveFileEx with MOVEFILE_REPLACE_EXISTING, so
        // there is no moment where the file is missing.
        fs::rename(&temp, &self.path)
    }
}

/// Move an unusable file aside so it can be inspected, never deleted.
fn preserve_corrupt(path: &Path) {
    let stamp = chrono::Utc::now().timestamp();
    let backup = path.with_file_name(format!("playtime.corrupt-{stamp}.json"));
    match fs::rename(path, &backup) {
        Ok(()) => warn!(
            "preserved the unreadable playtime file at {}; starting a fresh one",
            backup.display()
        ),
        Err(e) => error!("could not preserve the corrupt playtime file: {e}"),
    }
}

/// Bring an older file up to the current schema.
///
/// There is only one version so far, so this is a guard rather than a ladder:
/// a file from a *newer* ZougCloud build is left untouched and reported, since
/// downgrading and rewriting it would be the thing most likely to lose data.
fn migrate(mut state: PlaytimeState) -> PlaytimeState {
    if state.schema_version == SCHEMA_VERSION {
        return state;
    }

    if state.schema_version > SCHEMA_VERSION {
        warn!(
            "playtime file is schema v{} but this build understands v{SCHEMA_VERSION}; \
             reading it as-is and not rewriting the version",
            state.schema_version
        );
        return state;
    }

    info!(
        "migrating playtime file from schema v{} to v{SCHEMA_VERSION}",
        state.schema_version
    );
    state.schema_version = SCHEMA_VERSION;
    state
}
