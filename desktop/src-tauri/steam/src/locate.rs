use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use log::{debug, warn};
use serde::Serialize;

use crate::error::SteamError;

/// A Steam account that has a `userdata` directory on this machine.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SteamUser {
    /// The 32-bit account id, i.e. the `userdata/<id>` directory name.
    pub account_id: u32,
    /// Display name from `config/loginusers.vdf`, when we could read it.
    pub persona: Option<String>,
    /// The account Steam last signed in as. Used to pick a sensible default.
    pub most_recent: bool,
    pub userdata_dir: PathBuf,
}

impl SteamUser {
    /// `userdata/<id>/config/shortcuts.vdf` — Steam creates it on first use, so
    /// it legitimately may not exist yet.
    pub fn shortcuts_path(&self) -> PathBuf {
        self.userdata_dir.join("config").join("shortcuts.vdf")
    }

    /// `userdata/<id>/config/grid` — custom artwork for non-Steam shortcuts.
    pub fn grid_dir(&self) -> PathBuf {
        self.userdata_dir.join("config").join("grid")
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SteamInstall {
    pub path: PathBuf,
    pub users: Vec<SteamUser>,
    /// Steam rewrites shortcuts.vdf from memory when it exits, so writing while
    /// this is true would be silently undone. Callers shut Steam down first and
    /// restart it afterwards rather than refusing.
    pub running: bool,
}

/// Locate Steam and enumerate the accounts that have used it on this machine.
pub fn locate_steam() -> Result<SteamInstall, SteamError> {
    let dir = steamlocate::SteamDir::locate().map_err(|e| {
        debug!("steamlocate failed: {e}");
        SteamError::NotInstalled
    })?;

    let path = dir.path().to_path_buf();
    let users = enumerate_users(&path);

    if users.is_empty() {
        return Err(SteamError::NoUsers);
    }

    Ok(SteamInstall {
        path,
        users,
        running: is_steam_running(),
    })
}

fn enumerate_users(steam_path: &Path) -> Vec<SteamUser> {
    let userdata = steam_path.join("userdata");
    let entries = match std::fs::read_dir(&userdata) {
        Ok(e) => e,
        Err(e) => {
            warn!("could not read {}: {e}", userdata.display());
            return Vec::new();
        }
    };

    let logins = read_login_users(steam_path);

    let mut users: Vec<SteamUser> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Steam keeps an "anonymous" account directory (id 0) that cannot
            // own shortcuts. Parsing as u32 also discards any stray directory.
            let account_id: u32 = name.parse().ok()?;
            if account_id == 0 {
                return None;
            }
            let info = logins.iter().find(|(id, _, _)| *id == account_id);
            Some(SteamUser {
                account_id,
                persona: info.map(|(_, persona, _)| persona.clone()),
                most_recent: info.is_some_and(|(_, _, recent)| *recent),
                userdata_dir: entry.path(),
            })
        })
        .collect();

    // Most recently used first, then a stable order so the UI does not shuffle.
    users.sort_by(|a, b| {
        b.most_recent
            .cmp(&a.most_recent)
            .then(a.account_id.cmp(&b.account_id))
    });

    users
}

/// Steam's 64-bit ids for individual accounts are the 32-bit account id plus
/// this base. Subtracting it is how `userdata/<id>` relates to a SteamID64.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

/// Best-effort read of `config/loginusers.vdf` for display names.
///
/// Purely cosmetic: on any failure we fall back to bare account ids, which are
/// still perfectly usable. Returns `(account_id, persona, most_recent)`.
///
/// Picking "most recent" is deliberately defensive. Current Steam writes
/// `AutoLogin`; older builds wrote `MostRecent`; neither is guaranteed to be
/// present (a machine where nobody has auto-login enabled has neither). So we
/// accept either flag and otherwise fall back to the newest `Timestamp`, which
/// every entry carries.
fn read_login_users(steam_path: &Path) -> Vec<(u32, String, bool)> {
    let path = steam_path.join("config").join("loginusers.vdf");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            debug!("no loginusers.vdf ({e}); falling back to account ids");
            return Vec::new();
        }
    };

    let vdf = match keyvalues_parser::parse(&text) {
        Ok(v) => v,
        Err(e) => {
            warn!("could not parse loginusers.vdf: {e}");
            return Vec::new();
        }
    };

    let Some(root) = vdf.value.get_obj() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (id64, values) in root.iter() {
        let Ok(id64) = id64.parse::<u64>() else {
            continue;
        };
        let Some(account_id) = id64.checked_sub(STEAM_ID64_BASE) else {
            continue;
        };
        let Ok(account_id) = u32::try_from(account_id) else {
            continue;
        };

        let Some(fields) = values.first().and_then(|v| v.get_obj()) else {
            continue;
        };

        let field = |name: &str| -> Option<String> {
            fields
                .get(name)
                .and_then(|v| v.first())
                .and_then(|v| v.get_str())
                .map(str::to_owned)
        };

        let persona = field("PersonaName")
            .or_else(|| field("AccountName"))
            .unwrap_or_else(|| format!("Account {account_id}"));
        let explicit_recent = field("MostRecent").as_deref() == Some("1")
            || field("AutoLogin").as_deref() == Some("1");
        let timestamp = field("Timestamp")
            .and_then(|t| t.parse::<u64>().ok())
            .unwrap_or(0);

        out.push((account_id, persona, explicit_recent, timestamp));
    }

    // Nothing carried an explicit flag: fall back to the newest sign-in.
    if !out.iter().any(|(_, _, recent, _)| *recent)
        && let Some(newest) = out
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, _, _, ts))| *ts)
            .map(|(i, _)| i)
    {
        out[newest].2 = true;
    }

    out.into_iter()
        .map(|(id, persona, recent, _)| (id, persona, recent))
        .collect()
}

#[cfg(target_os = "windows")]
const STEAM_BINARY: &str = "steam.exe";
#[cfg(not(target_os = "windows"))]
const STEAM_BINARY: &str = "steam";

pub fn steam_executable(install: &SteamInstall) -> PathBuf {
    install.path.join(STEAM_BINARY)
}

/// Ask Steam to shut down cleanly.
///
/// `-shutdown` is Steam's own documented graceful exit: it flushes state,
/// closes the client and, importantly, refuses to go while a game is still
/// running. We never kill the process -- terminating Steam mid-write is exactly
/// how its config gets corrupted, which is the thing this whole module is
/// trying to avoid.
pub fn request_steam_exit(install: &SteamInstall) -> Result<(), SteamError> {
    let exe = steam_executable(install);
    debug!("asking Steam to shut down via {}", exe.display());
    std::process::Command::new(&exe).arg("-shutdown").spawn()?;
    Ok(())
}

/// Wait for Steam to actually be gone. Returns false on timeout.
///
/// Steam does not exit instantly, and it will not exit at all while a game is
/// running -- so the caller must treat a timeout as "tell the user", never as
/// "write anyway".
pub fn wait_for_steam_exit(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_steam_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    !is_steam_running()
}

/// Start Steam again after we are done editing its files.
pub fn launch_steam(install: &SteamInstall) -> Result<(), SteamError> {
    let exe = steam_executable(install);
    debug!("restarting Steam from {}", exe.display());
    std::process::Command::new(&exe).spawn()?;
    Ok(())
}

/// Is Steam running right now?
///
/// Anything we write to shortcuts.vdf while Steam is up is overwritten from
/// memory when it exits, so every write path deals with this first.
pub fn is_steam_running() -> bool {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System};

    let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );

    system.processes().values().any(|process| {
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        name == "steam.exe" || name == "steam"
    })
}
