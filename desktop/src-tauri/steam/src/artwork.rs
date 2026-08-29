//! ZOUGCLOUD(ZC-005): custom artwork for non-Steam shortcuts.
//!
//! Steam looks for artwork in `userdata/<account>/config/grid/`, named after the
//! shortcut's 32-bit app id. There is no index and no manifest: the filename
//! *is* the association. That is why ZC-006 goes to the trouble of preserving a
//! shortcut's app id across a move -- change the id and every one of these files
//! is orphaned.
//!
//! Steam only reads this directory at startup, so artwork written while Steam is
//! running does not appear until it is restarted.

use std::path::{Path, PathBuf};

use log::{debug, warn};
use serde::Serialize;

use crate::{error::SteamError, locate::SteamUser};

/// The five artwork slots Steam renders for a library entry.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ArtworkKind {
    /// Wide capsule, shown in the library grid list. `<appid>.png`
    Capsule,
    /// Vertical capsule, the main library tile. `<appid>p.png`
    Portrait,
    /// Banner behind the game's page. `<appid>_hero.png`
    Hero,
    /// Transparent title treatment drawn over the hero. `<appid>_logo.png`
    Logo,
    /// Small icon in lists and the shortcut itself. `<appid>_icon.png`
    Icon,
}

impl ArtworkKind {
    pub const ALL: [ArtworkKind; 5] = [
        ArtworkKind::Capsule,
        ArtworkKind::Portrait,
        ArtworkKind::Hero,
        ArtworkKind::Logo,
        ArtworkKind::Icon,
    ];

    /// The filename Steam looks for, without extension.
    ///
    /// Note that the capsule has no suffix at all, so its stem is a prefix of
    /// the portrait's (`123` vs `123p`). Every lookup here compares whole
    /// stems rather than prefixes, or removing one would take the other.
    pub fn stem(self, app_id: u32) -> String {
        match self {
            ArtworkKind::Capsule => format!("{app_id}"),
            ArtworkKind::Portrait => format!("{app_id}p"),
            ArtworkKind::Hero => format!("{app_id}_hero"),
            ArtworkKind::Logo => format!("{app_id}_logo"),
            ArtworkKind::Icon => format!("{app_id}_icon"),
        }
    }
}

/// Pick a file extension from the image's magic bytes.
///
/// Steam dispatches on the extension, so writing a JPEG as `.png` gives a
/// silently blank tile. Defaults to png when the format is unrecognised, which
/// is the format everything we fetch is converted to anyway.
pub fn detect_extension(bytes: &[u8]) -> &'static str {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const JPEG: &[u8] = b"\xFF\xD8\xFF";

    if bytes.starts_with(PNG) {
        "png"
    } else if bytes.starts_with(JPEG) {
        "jpg"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else {
        "png"
    }
}

fn grid_entries(grid: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(grid) {
        Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Delete every file whose stem matches, regardless of extension.
///
/// Steam picks whichever extension it finds, so leaving a stale `123.jpg`
/// behind while writing `123.png` would let the old image win.
fn clear_slot(grid: &Path, stem: &str) {
    for path in grid_entries(grid) {
        if path.file_stem().and_then(|s| s.to_str()) == Some(stem)
            && let Err(e) = std::fs::remove_file(&path)
        {
            warn!("could not replace artwork {}: {e}", path.display());
        }
    }
}

/// Write one artwork slot for a shortcut.
pub fn write_artwork(
    user: &SteamUser,
    app_id: u32,
    kind: ArtworkKind,
    bytes: &[u8],
) -> Result<PathBuf, SteamError> {
    let grid = user.grid_dir();
    std::fs::create_dir_all(&grid)?;

    let stem = kind.stem(app_id);
    clear_slot(&grid, &stem);

    let path = grid.join(format!("{stem}.{}", detect_extension(bytes)));
    std::fs::write(&path, bytes)?;
    debug!("wrote {:?} artwork to {}", kind, path.display());
    Ok(path)
}

/// Which slots currently have artwork on disk.
pub fn installed_artwork(user: &SteamUser, app_id: u32) -> Vec<ArtworkKind> {
    let entries = grid_entries(&user.grid_dir());
    ArtworkKind::ALL
        .into_iter()
        .filter(|kind| {
            let stem = kind.stem(app_id);
            entries
                .iter()
                .any(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str()))
        })
        .collect()
}

/// Remove every artwork slot for a shortcut. Returns how many files went.
///
/// Only ever called for a shortcut Drop manages: artwork sitting next to a
/// shortcut the user made by hand is theirs, not ours to delete.
pub fn remove_artwork(user: &SteamUser, app_id: u32) -> usize {
    let grid = user.grid_dir();
    let entries = grid_entries(&grid);
    let mut removed = 0;

    for kind in ArtworkKind::ALL {
        let stem = kind.stem(app_id);
        for path in &entries {
            if path.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str())
                && std::fs::remove_file(path).is_ok()
            {
                removed += 1;
            }
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(dir: &Path) -> SteamUser {
        SteamUser {
            account_id: 1,
            persona: None,
            most_recent: true,
            userdata_dir: dir.to_path_buf(),
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";
    const JPEG: &[u8] = b"\xFF\xD8\xFFrest";

    #[test]
    fn stems_match_steams_naming() {
        assert_eq!(ArtworkKind::Capsule.stem(42), "42");
        assert_eq!(ArtworkKind::Portrait.stem(42), "42p");
        assert_eq!(ArtworkKind::Hero.stem(42), "42_hero");
        assert_eq!(ArtworkKind::Logo.stem(42), "42_logo");
        assert_eq!(ArtworkKind::Icon.stem(42), "42_icon");
    }

    #[test]
    fn extension_follows_the_magic_bytes() {
        assert_eq!(detect_extension(PNG), "png");
        assert_eq!(detect_extension(JPEG), "jpg");
        assert_eq!(detect_extension(b"RIFF____WEBPxxxx"), "webp");
        assert_eq!(detect_extension(b"nonsense"), "png");
    }

    #[test]
    fn writing_then_listing_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());

        write_artwork(&u, 42, ArtworkKind::Portrait, PNG).expect("write");
        assert_eq!(installed_artwork(&u, 42), vec![ArtworkKind::Portrait]);
        assert!(u.grid_dir().join("42p.png").is_file());
    }

    #[test]
    fn the_capsule_slot_does_not_collide_with_the_portrait() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());

        write_artwork(&u, 42, ArtworkKind::Capsule, PNG).expect("capsule");
        write_artwork(&u, 42, ArtworkKind::Portrait, PNG).expect("portrait");

        // "42" is a prefix of "42p": prefix matching here would have deleted one.
        assert!(u.grid_dir().join("42.png").is_file());
        assert!(u.grid_dir().join("42p.png").is_file());
        assert_eq!(installed_artwork(&u, 42).len(), 2);
    }

    #[test]
    fn rewriting_a_slot_replaces_a_different_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());

        write_artwork(&u, 42, ArtworkKind::Portrait, JPEG).expect("jpeg");
        assert!(u.grid_dir().join("42p.jpg").is_file());

        write_artwork(&u, 42, ArtworkKind::Portrait, PNG).expect("png");
        assert!(u.grid_dir().join("42p.png").is_file());
        // A leftover .jpg would win in Steam, so it must be gone.
        assert!(!u.grid_dir().join("42p.jpg").exists());
        assert_eq!(installed_artwork(&u, 42).len(), 1);
    }

    #[test]
    fn removing_takes_every_slot_and_leaves_other_games_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());

        for kind in ArtworkKind::ALL {
            write_artwork(&u, 42, kind, PNG).expect("write");
        }
        write_artwork(&u, 99, ArtworkKind::Portrait, PNG).expect("other game");

        assert_eq!(remove_artwork(&u, 42), 5);
        assert!(installed_artwork(&u, 42).is_empty());
        assert_eq!(installed_artwork(&u, 99), vec![ArtworkKind::Portrait]);
    }

    #[test]
    fn listing_a_missing_grid_dir_is_empty_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let u = user(dir.path());
        assert!(installed_artwork(&u, 42).is_empty());
        assert_eq!(remove_artwork(&u, 42), 0);
    }
}
