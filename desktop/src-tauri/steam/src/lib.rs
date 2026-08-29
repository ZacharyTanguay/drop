//! ZOUGCLOUD(ZC-004): optional Steam integration for Drop Desktop.
//!
//! Drop stays the installer, updater and version manager. Steam optionally
//! becomes the launcher, giving members the overlay, controller support and
//! playtime tracking they already expect — without Drop having to build any of
//! it.
//!
//! The shortcut always points at the **game** executable, never at
//! `drop-app.exe`: routing through the launcher would make Steam record
//! playtime for Drop instead of for the game, which defeats the point.
//!
//! Entirely client-side. Nothing here talks to a Drop server; it reads Steam's
//! own files under `userdata/<account>/config/` and, optionally, SteamGridDB.
//! Real Steam licences live in `steamapps/appmanifest_*.acf` and are never
//! opened, so a genuine copy of the same game cannot be affected.

pub mod error;
pub mod locate;
pub mod shortcuts;

pub use error::SteamError;
pub use locate::{SteamInstall, SteamUser, is_steam_running, locate_steam};
pub use shortcuts::{
    DROP_TAG, ShortcutRecord, ShortcutRequest, app_id_for, find_shortcut, list_shortcuts,
    remove_shortcut, run_game_id, steam_quote, upsert_shortcut,
};
