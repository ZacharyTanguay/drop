use std::path::{Path, PathBuf};

use log::{debug, info, warn};
use serde::Serialize;
use steam_shortcuts_util::{
    Shortcut, app_id_generator::calculate_app_id, parse_shortcuts, shortcut::ShortcutOwned,
    shortcuts_to_bytes,
};

use crate::{error::SteamError, locate::SteamUser};

/// Tag written on every shortcut we create. It doubles as a Steam category, so
/// members get a "Drop" shelf for free, and as the marker that lets us tell our
/// own entries apart from shortcuts the user added by hand.
pub const DROP_TAG: &str = "Drop";

/// How many rolling backups of shortcuts.vdf to keep per account.
const BACKUPS_TO_KEEP: usize = 5;

#[derive(Debug, Clone)]
pub struct ShortcutRequest {
    pub app_name: String,
    /// Absolute path to the **game** executable. Never drop-app.exe: Steam must
    /// time the game, not the launcher.
    pub exe: PathBuf,
    pub start_dir: PathBuf,
    pub launch_options: String,
    pub icon: Option<PathBuf>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutRecord {
    pub app_id: u32,
    pub app_name: String,
    pub exe: String,
    pub start_dir: String,
    pub launch_options: String,
    /// `steam://rungameid/<id>` — opens the game's page in Steam.
    pub run_game_id: String,
    /// False for a shortcut the user created by hand that we matched onto.
    pub managed_by_drop: bool,
}

impl ShortcutRecord {
    fn from_shortcut(s: &ShortcutOwned) -> Self {
        Self {
            app_id: s.app_id,
            app_name: s.app_name.clone(),
            exe: s.exe.clone(),
            start_dir: s.start_dir.clone(),
            launch_options: s.launch_options.clone(),
            run_game_id: run_game_id(s.app_id).to_string(),
            managed_by_drop: s.tags.iter().any(|t| t == DROP_TAG),
        }
    }
}

/// Steam stores `exe` and `start_dir` wrapped in literal double quotes.
///
/// This is not cosmetic: the app id is a hash of this exact string, so quoting
/// inconsistently would produce a different id for the same game and orphan its
/// artwork and playtime.
pub fn steam_quote(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// The 32-bit id Steam uses for a non-Steam shortcut, and the one its custom
/// artwork files are named after.
pub fn app_id_for(exe_quoted: &str, app_name: &str) -> u32 {
    calculate_app_id(exe_quoted, app_name)
}

/// The 64-bit id for `steam://rungameid/`.
pub fn run_game_id(app_id: u32) -> u64 {
    (u64::from(app_id) << 32) | 0x0200_0000
}

fn normalise_exe(exe: &str) -> String {
    exe.trim_matches('"').replace('/', "\\").to_ascii_lowercase()
}

/// Read an account's shortcuts. A missing file is not an error: Steam only
/// creates it once the account has at least one non-Steam shortcut.
pub fn read_shortcuts(path: &Path) -> Result<Vec<ShortcutOwned>, SteamError> {
    if !path.exists() {
        debug!("no shortcuts.vdf at {}, treating as empty", path.display());
        return Ok(Vec::new());
    }

    let bytes = std::fs::read(path)?;
    let parsed =
        parse_shortcuts(bytes.as_slice()).map_err(|e| SteamError::ShortcutsUnreadable(e))?;

    Ok(parsed.iter().map(Shortcut::to_owned).collect())
}

/// Write shortcuts back, keeping a rolling backup and swapping the file in
/// atomically.
///
/// Steam's shortcuts file is the user's own data. A truncated write would lose
/// every non-Steam shortcut they have, including ones Drop never created, so we
/// build the whole file in memory, write it beside the original and rename over
/// the top -- a crash mid-write leaves the original intact.
pub fn write_shortcuts(path: &Path, shortcuts: &[ShortcutOwned]) -> Result<(), SteamError> {
    let parent = path
        .parent()
        .ok_or_else(|| SteamError::ShortcutsUnwritable(format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(parent)?;

    // Renumber so `order` matches position; Steam keys entries on it.
    let renumbered: Vec<ShortcutOwned> = shortcuts
        .iter()
        .enumerate()
        .map(|(index, s)| {
            let mut s = s.clone();
            s.order = index.to_string();
            s
        })
        .collect();

    let borrowed: Vec<Shortcut<'_>> = renumbered.iter().map(ShortcutOwned::borrow).collect();
    let bytes = shortcuts_to_bytes(&borrowed);

    if path.exists() {
        back_up(path)?;
    }

    let temp = path.with_extension("vdf.drop-tmp");
    std::fs::write(&temp, &bytes)?;
    // On Windows this maps to MoveFileEx with MOVEFILE_REPLACE_EXISTING, so the
    // swap is atomic and no window exists where the file is missing.
    std::fs::rename(&temp, path)?;

    info!(
        "wrote {} shortcut(s) to {}",
        renumbered.len(),
        path.display()
    );
    Ok(())
}

fn back_up(path: &Path) -> Result<(), SteamError> {
    let stamp = chrono::Utc::now().timestamp();
    let backup = path.with_extension(format!("vdf.drop-backup-{stamp}"));
    std::fs::copy(path, &backup)?;
    debug!("backed up shortcuts to {}", backup.display());
    prune_backups(path);
    Ok(())
}

/// Keep only the newest `BACKUPS_TO_KEEP` backups. Best-effort: failing to tidy
/// up is never a reason to fail the operation the user asked for.
fn prune_backups(path: &Path) {
    let Some(parent) = path.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };

    let mut backups: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("shortcuts.vdf.drop-backup-"))
        })
        .collect();

    if backups.len() <= BACKUPS_TO_KEEP {
        return;
    }

    backups.sort();
    for old in &backups[..backups.len() - BACKUPS_TO_KEEP] {
        if let Err(e) = std::fs::remove_file(old) {
            warn!("could not prune backup {}: {e}", old.display());
        }
    }
}

/// Locate an existing entry for this game.
///
/// Three strategies, most to least precise:
///  1. the app id we would generate now — the ordinary case;
///  2. a Drop-tagged shortcut with the same name — this is what catches a game
///     that moved to a different install directory, where the exe changed and
///     therefore so did the generated id;
///  3. the same executable — catches a game renamed inside Drop.
fn find_existing(
    shortcuts: &[ShortcutOwned],
    app_id: u32,
    app_name: &str,
    exe_quoted: &str,
) -> Option<usize> {
    if let Some(i) = shortcuts.iter().position(|s| s.app_id == app_id) {
        return Some(i);
    }

    let normalised = normalise_exe(exe_quoted);

    if let Some(i) = shortcuts.iter().position(|s| {
        s.tags.iter().any(|t| t == DROP_TAG) && s.app_name == app_name
    }) {
        return Some(i);
    }

    shortcuts
        .iter()
        .position(|s| normalise_exe(&s.exe) == normalised)
}

/// Add the game to Steam, or update the entry that is already there.
///
/// Never creates a second entry for a game that is already present, and never
/// touches a real Steam licence: shortcuts.vdf holds *only* non-Steam
/// shortcuts. An owned copy of the same game lives in
/// `steamapps/appmanifest_<id>.acf`, which this code never opens.
pub fn upsert_shortcut(
    user: &SteamUser,
    request: &ShortcutRequest,
) -> Result<ShortcutRecord, SteamError> {
    if !request.exe.is_file() {
        return Err(SteamError::ExecutableMissing(
            request.exe.display().to_string(),
        ));
    }

    let path = user.shortcuts_path();
    let mut shortcuts = read_shortcuts(&path)?;

    let exe_quoted = steam_quote(&request.exe);
    let start_dir_quoted = steam_quote(&request.start_dir);
    let icon = request
        .icon
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let fresh_app_id = app_id_for(&exe_quoted, &request.app_name);

    let index = match find_existing(&shortcuts, fresh_app_id, &request.app_name, &exe_quoted) {
        Some(index) => {
            let existing = &mut shortcuts[index];

            // ZOUGCLOUD(ZC-006): keep the *existing* app id even when the
            // executable moved and the generated id would now differ. Steam keys
            // playtime on the id in this file and never recomputes it, and the
            // artwork files are named after it -- so reusing it is what lets a
            // Drop update preserve both. Only the target is refreshed.
            info!(
                "updating existing Steam shortcut {} ({})",
                existing.app_id, existing.app_name
            );
            existing.app_name = request.app_name.clone();
            existing.exe = exe_quoted;
            existing.start_dir = start_dir_quoted;
            existing.launch_options = request.launch_options.clone();
            if !icon.is_empty() {
                existing.icon = icon;
            }
            if !existing.tags.iter().any(|t| t == DROP_TAG) {
                existing.tags.push(DROP_TAG.to_owned());
            }
            index
        }
        None => {
            info!(
                "adding Steam shortcut {} ({})",
                fresh_app_id, request.app_name
            );
            let mut shortcut = Shortcut::new(
                "0",
                &request.app_name,
                &exe_quoted,
                &start_dir_quoted,
                &icon,
                "",
                &request.launch_options,
            )
            .to_owned();
            shortcut.app_id = fresh_app_id;
            shortcut.tags.push(DROP_TAG.to_owned());
            shortcuts.push(shortcut);
            shortcuts.len() - 1
        }
    };

    write_shortcuts(&path, &shortcuts)?;

    Ok(ShortcutRecord::from_shortcut(&shortcuts[index]))
}

/// Find the Steam shortcut for a game, if it has one.
pub fn find_shortcut(
    user: &SteamUser,
    app_name: &str,
    exe: &Path,
) -> Result<Option<ShortcutRecord>, SteamError> {
    let shortcuts = read_shortcuts(&user.shortcuts_path())?;
    let exe_quoted = steam_quote(exe);
    let app_id = app_id_for(&exe_quoted, app_name);

    Ok(find_existing(&shortcuts, app_id, app_name, &exe_quoted)
        .map(|i| ShortcutRecord::from_shortcut(&shortcuts[i])))
}

/// Remove a shortcut. Only ever called explicitly by the user -- a game update
/// must never silently drop someone's Steam entry.
pub fn remove_shortcut(user: &SteamUser, app_id: u32) -> Result<(), SteamError> {
    let path = user.shortcuts_path();
    let mut shortcuts = read_shortcuts(&path)?;

    let before = shortcuts.len();
    shortcuts.retain(|s| s.app_id != app_id);

    if shortcuts.len() == before {
        return Err(SteamError::NotShortcut(app_id));
    }

    write_shortcuts(&path, &shortcuts)
}

pub fn list_shortcuts(user: &SteamUser) -> Result<Vec<ShortcutRecord>, SteamError> {
    Ok(read_shortcuts(&user.shortcuts_path())?
        .iter()
        .map(ShortcutRecord::from_shortcut)
        .collect())
}

// ZOUGCLOUD(ZC-004): these tests exercise the shortcuts.vdf round trip against
// real files. Corrupting this file would lose a member's entire non-Steam
// library, so the write path is covered rather than trusted.
#[cfg(test)]
mod tests {
    use super::*;

    fn user(dir: &Path) -> SteamUser {
        SteamUser {
            account_id: 123,
            persona: None,
            most_recent: true,
            userdata_dir: dir.to_path_buf(),
        }
    }

    fn request(exe: &Path, name: &str) -> ShortcutRequest {
        ShortcutRequest {
            app_name: name.to_owned(),
            exe: exe.to_path_buf(),
            start_dir: exe.parent().unwrap().to_path_buf(),
            launch_options: String::new(),
            icon: None,
        }
    }

    fn make_exe(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"x").expect("write exe");
        path
    }

    #[test]
    fn app_id_is_stable_and_in_steams_range() {
        let a = app_id_for("\"C:\\games\\Game.exe\"", "Game");
        let b = app_id_for("\"C:\\games\\Game.exe\"", "Game");
        assert_eq!(a, b, "same input must give the same id");
        assert_ne!(a, app_id_for("\"C:\\games\\Other.exe\"", "Game"));
        assert_ne!(a, app_id_for("\"C:\\games\\Game.exe\"", "Other"));
        // Steam expects the top bit set on shortcut ids.
        assert_eq!(a & 0x8000_0000, 0x8000_0000);
    }

    #[test]
    fn run_game_id_has_the_shortcut_marker() {
        assert_eq!(run_game_id(0x8000_0001), 0x8000_0001_0200_0000);
    }

    #[test]
    fn quoting_matches_steams_convention() {
        assert_eq!(
            steam_quote(Path::new("C:\\games\\Graveyard Keeper.exe")),
            "\"C:\\games\\Graveyard Keeper.exe\""
        );
    }

    #[test]
    fn adding_then_reading_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let exe = make_exe(dir.path(), "Graveyard Keeper.exe");

        let record = upsert_shortcut(&u, &request(&exe, "Graveyard Keeper")).expect("add");
        assert!(record.managed_by_drop);

        let listed = list_shortcuts(&u).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].app_name, "Graveyard Keeper");
        assert_eq!(listed[0].app_id, record.app_id);
        assert!(listed[0].exe.contains("Graveyard Keeper.exe"));
    }

    #[test]
    fn adding_twice_does_not_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let exe = make_exe(dir.path(), "Game.exe");

        let first = upsert_shortcut(&u, &request(&exe, "Game")).expect("add");
        let second = upsert_shortcut(&u, &request(&exe, "Game")).expect("re-add");

        assert_eq!(first.app_id, second.app_id);
        assert_eq!(list_shortcuts(&u).expect("list").len(), 1);
    }

    #[test]
    fn a_moved_game_keeps_its_app_id_and_playtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let old_exe = make_exe(dir.path(), "Game.exe");

        let original = upsert_shortcut(&u, &request(&old_exe, "Game")).expect("add");

        // Simulate Steam having recorded playtime against the entry.
        let mut shortcuts = read_shortcuts(&u.shortcuts_path()).expect("read");
        shortcuts[0].last_play_time = 1_700_000_000;
        write_shortcuts(&u.shortcuts_path(), &shortcuts).expect("write");

        // Drop reinstalls the game somewhere else.
        let moved_dir = dir.path().join("v2");
        std::fs::create_dir_all(&moved_dir).expect("mkdir");
        let new_exe = make_exe(&moved_dir, "Game.exe");

        let updated = upsert_shortcut(&u, &request(&new_exe, "Game")).expect("update");

        assert_eq!(
            updated.app_id, original.app_id,
            "app id must survive a move, or Steam loses playtime and artwork"
        );
        let listed = list_shortcuts(&u).expect("list");
        assert_eq!(listed.len(), 1, "must update in place, not add a second entry");
        assert!(listed[0].exe.contains("v2"), "target must be refreshed");

        let shortcuts = read_shortcuts(&u.shortcuts_path()).expect("read");
        assert_eq!(shortcuts[0].last_play_time, 1_700_000_000);
    }

    #[test]
    fn unrelated_shortcuts_are_preserved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let mine = make_exe(dir.path(), "Game.exe");
        let theirs = make_exe(dir.path(), "SomethingElse.exe");

        // A shortcut the user added by hand, with no Drop tag.
        let theirs_quoted = steam_quote(&theirs);
        let mut hand_made = Shortcut::new("0", "Hand Made", &theirs_quoted, "", "", "", "").to_owned();
        hand_made.app_id = app_id_for(&theirs_quoted, "Hand Made");
        write_shortcuts(&u.shortcuts_path(), &[hand_made]).expect("seed");

        upsert_shortcut(&u, &request(&mine, "Game")).expect("add");

        let listed = list_shortcuts(&u).expect("list");
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|s| s.app_name == "Hand Made" && !s.managed_by_drop));
        assert!(listed.iter().any(|s| s.app_name == "Game" && s.managed_by_drop));
    }

    #[test]
    fn removing_only_removes_the_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let one = make_exe(dir.path(), "One.exe");
        let two = make_exe(dir.path(), "Two.exe");

        let a = upsert_shortcut(&u, &request(&one, "One")).expect("add one");
        upsert_shortcut(&u, &request(&two, "Two")).expect("add two");

        remove_shortcut(&u, a.app_id).expect("remove");

        let listed = list_shortcuts(&u).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].app_name, "Two");
    }

    #[test]
    fn removing_something_absent_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        assert!(matches!(
            remove_shortcut(&u, 12345),
            Err(SteamError::NotShortcut(12345))
        ));
    }

    #[test]
    fn a_missing_executable_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let ghost = dir.path().join("NotThere.exe");

        assert!(matches!(
            upsert_shortcut(&u, &request(&ghost, "Ghost")),
            Err(SteamError::ExecutableMissing(_))
        ));
    }

    #[test]
    fn writing_creates_a_backup_of_the_previous_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let exe = make_exe(dir.path(), "Game.exe");

        upsert_shortcut(&u, &request(&exe, "Game")).expect("add");
        upsert_shortcut(&u, &request(&exe, "Game Renamed")).expect("update");

        let config = dir.path().join("config");
        let backups = std::fs::read_dir(&config)
            .expect("read config")
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("shortcuts.vdf.drop-backup-")
            })
            .count();
        assert!(backups >= 1, "the previous file must be recoverable");
    }

    #[test]
    fn find_reports_absence_and_presence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        let exe = make_exe(dir.path(), "Game.exe");

        assert!(find_shortcut(&u, "Game", &exe).expect("find").is_none());
        upsert_shortcut(&u, &request(&exe, "Game")).expect("add");
        assert!(find_shortcut(&u, "Game", &exe).expect("find").is_some());
    }
}
