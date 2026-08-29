use std::{
    fmt::Display,
    io::{self},
    sync::Arc,
};

use serde_with::SerializeDisplay;

/// ZOUGCLOUD(ZC-004): mirrors the shape of `process::error::ProcessError` so the
/// frontend handles Steam failures exactly like every other Drop error.
#[derive(SerializeDisplay, Clone, Debug)]
pub enum SteamError {
    NotInstalled,
    NoUsers,
    UnknownUser(u32),
    /// We asked Steam to shut down and it was still there when we gave up.
    /// Steam refuses `-shutdown` while a game is running, which is the usual
    /// cause. Writing anyway would be silently undone when it finally exits.
    SteamWillNotClose,
    ShortcutsUnreadable(String),
    ShortcutsUnwritable(String),
    ExecutableMissing(String),
    NotShortcut(u32),
    Io(Arc<io::Error>),
}

impl Display for SteamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SteamError::NotInstalled => "Steam could not be found on this computer".to_owned(),
            SteamError::NoUsers => {
                "No Steam account was found on this computer. Sign in to Steam at least once."
                    .to_owned()
            }
            SteamError::UnknownUser(id) => format!("Unknown Steam account {id}"),
            SteamError::SteamWillNotClose => {
                "Steam would not close. This usually means a game is still running through \
                 Steam -- quit it, then try again."
                    .to_owned()
            }
            SteamError::ShortcutsUnreadable(e) => {
                format!("Could not read Steam's shortcuts file: {e}")
            }
            SteamError::ShortcutsUnwritable(e) => {
                format!("Could not write Steam's shortcuts file: {e}")
            }
            SteamError::ExecutableMissing(p) => {
                format!("The game executable does not exist: {p}")
            }
            SteamError::NotShortcut(id) => {
                format!("No Steam shortcut with id {id} was found for this account")
            }
            SteamError::Io(e) => e.to_string(),
        };
        write!(f, "{s}")
    }
}

impl From<io::Error> for SteamError {
    fn from(value: io::Error) -> Self {
        SteamError::Io(Arc::new(value))
    }
}
