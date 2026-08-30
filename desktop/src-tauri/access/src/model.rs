use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Bump when the on-disk/on-remote shape changes.
pub const SCHEMA_VERSION: u32 = 1;

/// The Drop username that always bypasses every access rule.
///
/// This is UX steering, not a security boundary: the check runs in the client,
/// and anyone technical could use the stock Drop client instead. It exists to
/// shape what non-technical members see.
pub const ADMIN_USERNAME: &str = "zacktanguay";

/// How a game is made available.
///
/// `Free` and a price of zero are deliberately *not* the same thing: a game can
/// be gated with no price configured yet, and a free game never needs a price.
/// Conflating them would make "no price set" silently grant access.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    /// Available to every member without an individual grant.
    Free,
    /// Requires an explicit per-member grant.
    Gated,
    /// Always available to everyone, overriding any per-user configuration —
    /// including an absent one. Used for games that must never be hidden.
    All,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    /// Minor units: 1299 is $12.99. Never a float — money in binary floating
    /// point accumulates error and cannot represent 0.10 exactly.
    pub amount_minor: i64,
    /// ISO 4217, e.g. "CAD". Stored even though the UI is CAD-only today.
    pub currency: String,
}

/// Deserialise into `Some(T)`, or `None` when the value is absent *or*
/// unrecognised.
///
/// Plain `Option<T>` only covers absence: an unknown `accessMode` written by a
/// newer ZougCloud would fail the whole manifest, so one bad game policy would
/// drop every grant. Degrading that single field instead keeps the rest of the
/// manifest usable, and `None` still fails closed for the affected game.
fn lenient_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GamePolicy {
    #[serde(default, deserialize_with = "lenient_option")]
    pub access_mode: Option<AccessMode>,
    #[serde(default, deserialize_with = "lenient_option")]
    pub price: Option<Price>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserGrants {
    #[serde(default)]
    pub allowed_games: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AccessManifest {
    pub schema_version: u32,
    /// Increases on every admin change. Clients poll this to notice updates.
    pub revision: u64,
    #[serde(default)]
    pub games: HashMap<String, GamePolicy>,
    /// Keyed by Drop `User.id` (a UUID), never by username: usernames are
    /// guessable and mutable, the UUID is neither.
    #[serde(default)]
    pub users: HashMap<String, UserGrants>,
}

impl Default for AccessManifest {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            revision: 0,
            games: HashMap::new(),
            users: HashMap::new(),
        }
    }
}

/// Who is asking. Both identifiers are needed: the username decides admin
/// bypass, the UUID keys the grants.
#[derive(Clone, Debug)]
pub struct Viewer {
    pub user_id: String,
    pub username: String,
}

impl Viewer {
    pub fn is_admin(&self) -> bool {
        self.username.eq_ignore_ascii_case(ADMIN_USERNAME)
    }
}

/// Why a game is or is not available — the UI needs the reason, not just a
/// boolean, to choose between "Add to Library", a price plus "I'm interested",
/// and hiding the entry outright.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum AccessDecision {
    /// Admin bypass.
    AllowedAsAdmin,
    /// `accessMode: all` — never hidden, never gated.
    AllowedForEveryone,
    /// `accessMode: free`.
    AllowedAsFree,
    /// `accessMode: gated` with an explicit grant.
    AllowedByGrant,
    /// `accessMode: gated` without a grant. The only state that offers
    /// "I'm interested".
    DeniedGated,
    /// No policy, or one this build cannot understand. Fails closed.
    DeniedUnknownPolicy,
}

impl AccessDecision {
    pub fn is_allowed(self) -> bool {
        !matches!(
            self,
            AccessDecision::DeniedGated | AccessDecision::DeniedUnknownPolicy
        )
    }

    /// Interest only ever applies to a gated game the member cannot have.
    pub fn offers_interest(self) -> bool {
        matches!(self, AccessDecision::DeniedGated)
    }
}
