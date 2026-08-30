//! ZOUGCLOUD(ZC-011/ZC-013): who may have which game.
//!
//! **This is UX steering, not authorisation.** The decision runs entirely in
//! the client, so anyone technical could bypass it with the stock Drop client
//! or by calling the server API directly. It exists to shape what
//! non-technical members see, and the Drop server is deliberately unchanged.
//!
//! Everything funnels through [`decide`] so the library filter and the Store
//! proxy guard can never drift apart. Adding a new surface means calling this,
//! not writing another condition.

#![feature(nonpoison_mutex)]
#![feature(sync_nonpoison)]

pub mod model;
pub mod state;
pub mod store;

pub use model::{
    ADMIN_USERNAME, AccessDecision, AccessManifest, AccessMode, GamePolicy, Price, SCHEMA_VERSION,
    UserGrants, Viewer,
};
pub use state::ACCESS;
pub use store::ManifestCache;

/// Decide whether `viewer` may have `game_id`.
///
/// Precedence, highest first:
/// 1. the admin bypasses everything;
/// 2. `all` overrides every per-user rule, including a missing user entry;
/// 3. `free` needs no grant;
/// 4. `gated` needs an explicit grant;
/// 5. anything else fails closed.
pub fn decide(manifest: &AccessManifest, viewer: &Viewer, game_id: &str) -> AccessDecision {
    if viewer.is_admin() {
        return AccessDecision::AllowedAsAdmin;
    }

    let Some(policy) = manifest.games.get(game_id) else {
        // A game with no policy is not implicitly free: a member must never
        // gain access because an admin forgot to configure something.
        return AccessDecision::DeniedUnknownPolicy;
    };

    match policy.access_mode {
        // Checked before any user lookup, which is what makes `all` immune to
        // a member being absent from the manifest or explicitly ungranted.
        Some(AccessMode::All) => AccessDecision::AllowedForEveryone,
        Some(AccessMode::Free) => AccessDecision::AllowedAsFree,
        Some(AccessMode::Gated) => {
            if has_grant(manifest, &viewer.user_id, game_id) {
                AccessDecision::AllowedByGrant
            } else {
                AccessDecision::DeniedGated
            }
        }
        // An accessMode this build does not understand, e.g. written by a
        // newer ZougCloud. Fail closed for members; the admin still bypasses.
        None => AccessDecision::DeniedUnknownPolicy,
    }
}

fn has_grant(manifest: &AccessManifest, user_id: &str, game_id: &str) -> bool {
    manifest
        .users
        .get(user_id)
        .is_some_and(|grants| grants.allowed_games.iter().any(|id| id == game_id))
}

pub fn is_game_accessible(manifest: &AccessManifest, viewer: &Viewer, game_id: &str) -> bool {
    decide(manifest, viewer, game_id).is_allowed()
}

/// Keep only the games this viewer may have.
///
/// `key` maps an item to its stable Drop game id, so callers can filter their
/// own types without converting first.
pub fn filter_accessible<T, F>(
    manifest: &AccessManifest,
    viewer: &Viewer,
    items: Vec<T>,
    key: F,
) -> Vec<T>
where
    F: Fn(&T) -> String,
{
    items
        .into_iter()
        .filter(|item| is_game_accessible(manifest, viewer, &key(item)))
        .collect()
}

/// The price to display, if one is configured.
///
/// `None` means "no price set", which is not the same as free — a gated game
/// can be priceless and still require a grant.
pub fn price_for<'a>(manifest: &'a AccessManifest, game_id: &str) -> Option<&'a Price> {
    manifest.games.get(game_id)?.price.as_ref()
}

/// Render minor units for display: 1299 → "$12.99 CAD".
pub fn format_price(price: &Price) -> String {
    let sign = if price.amount_minor < 0 { "-" } else { "" };
    let abs = price.amount_minor.unsigned_abs();
    format!("{sign}${}.{:02} {}", abs / 100, abs % 100, price.currency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const GAME: &str = "11111111-1111-1111-1111-111111111111";
    const OTHER: &str = "22222222-2222-2222-2222-222222222222";
    const BOB: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    const MARC: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";

    fn member(id: &str) -> Viewer {
        Viewer {
            user_id: id.to_owned(),
            username: "bob".to_owned(),
        }
    }

    fn admin() -> Viewer {
        Viewer {
            user_id: "aaaa".to_owned(),
            username: ADMIN_USERNAME.to_owned(),
        }
    }

    fn manifest(mode: Option<AccessMode>, grants: &[(&str, &[&str])]) -> AccessManifest {
        let mut games = HashMap::new();
        games.insert(
            GAME.to_owned(),
            GamePolicy {
                access_mode: mode,
                price: None,
            },
        );
        let mut users = HashMap::new();
        for (user, allowed) in grants {
            users.insert(
                (*user).to_owned(),
                UserGrants {
                    allowed_games: allowed.iter().map(|g| (*g).to_owned()).collect(),
                },
            );
        }
        AccessManifest {
            games,
            users,
            ..Default::default()
        }
    }

    // --- admin ------------------------------------------------------------

    #[test]
    fn the_admin_bypasses_every_rule() {
        for mode in [None, Some(AccessMode::Gated), Some(AccessMode::Free)] {
            let m = manifest(mode, &[]);
            assert_eq!(decide(&m, &admin(), GAME), AccessDecision::AllowedAsAdmin);
        }
        // Including a game with no policy at all.
        let empty = AccessManifest::default();
        assert!(is_game_accessible(&empty, &admin(), GAME));
    }

    #[test]
    fn the_admin_check_is_case_insensitive() {
        let v = Viewer {
            user_id: "x".to_owned(),
            username: "ZackTanguay".to_owned(),
        };
        assert!(v.is_admin());
    }

    // --- free -------------------------------------------------------------

    #[test]
    fn a_free_game_needs_no_grant() {
        let m = manifest(Some(AccessMode::Free), &[]);
        let d = decide(&m, &member(BOB), GAME);
        assert_eq!(d, AccessDecision::AllowedAsFree);
        assert!(d.is_allowed());
        assert!(!d.offers_interest(), "free games never offer interest");
    }

    // --- all --------------------------------------------------------------

    #[test]
    fn an_all_game_is_available_to_a_member_with_no_manifest_entry() {
        let m = manifest(Some(AccessMode::All), &[]);
        let d = decide(&m, &member("unknown-brand-new-member"), GAME);
        assert_eq!(d, AccessDecision::AllowedForEveryone);
        assert!(!d.offers_interest());
    }

    #[test]
    fn a_user_entry_cannot_override_all() {
        // Bob exists in the manifest and is granted only OTHER: `all` must
        // still win. This is the precedence the mandate requires.
        let m = manifest(Some(AccessMode::All), &[(BOB, &[OTHER])]);
        assert_eq!(
            decide(&m, &member(BOB), GAME),
            AccessDecision::AllowedForEveryone
        );
    }

    // --- gated ------------------------------------------------------------

    #[test]
    fn a_gated_game_without_a_grant_is_denied_and_offers_interest() {
        let m = manifest(Some(AccessMode::Gated), &[]);
        let d = decide(&m, &member(BOB), GAME);
        assert_eq!(d, AccessDecision::DeniedGated);
        assert!(!d.is_allowed());
        assert!(d.offers_interest());
    }

    #[test]
    fn a_gated_game_with_a_grant_is_allowed() {
        let m = manifest(Some(AccessMode::Gated), &[(BOB, &[GAME])]);
        let d = decide(&m, &member(BOB), GAME);
        assert_eq!(d, AccessDecision::AllowedByGrant);
        assert!(!d.offers_interest(), "an allowed game never offers interest");
    }

    #[test]
    fn grants_do_not_leak_between_members() {
        let m = manifest(Some(AccessMode::Gated), &[(BOB, &[GAME])]);
        assert!(is_game_accessible(&m, &member(BOB), GAME));
        assert!(!is_game_accessible(&m, &member(MARC), GAME));
    }

    #[test]
    fn grants_do_not_leak_between_games() {
        let m = manifest(Some(AccessMode::Gated), &[(BOB, &[OTHER])]);
        assert!(!is_game_accessible(&m, &member(BOB), GAME));
    }

    // --- failing closed ---------------------------------------------------

    #[test]
    fn a_game_with_no_policy_is_denied_for_members() {
        let m = AccessManifest::default();
        assert_eq!(
            decide(&m, &member(BOB), GAME),
            AccessDecision::DeniedUnknownPolicy
        );
    }

    #[test]
    fn an_unreadable_access_mode_fails_closed() {
        // accessMode written by a newer build: deserialises to None.
        let m = manifest(None, &[(BOB, &[GAME])]);
        let d = decide(&m, &member(BOB), GAME);
        assert_eq!(d, AccessDecision::DeniedUnknownPolicy);
        // And it must not offer interest -- we do not know it is gated.
        assert!(!d.offers_interest());
    }

    #[test]
    fn an_unknown_access_mode_string_deserialises_to_none() {
        let json = r#"{"schemaVersion":1,"revision":1,
            "games":{"g":{"accessMode":"subscription"}},"users":{}}"#;
        let parsed: AccessManifest = serde_json::from_str(json).expect("parse");
        assert!(parsed.games["g"].access_mode.is_none());
    }

    // --- filtering --------------------------------------------------------

    #[test]
    fn filtering_keeps_only_what_a_member_may_have() {
        let mut m = manifest(Some(AccessMode::Free), &[]);
        m.games.insert(
            OTHER.to_owned(),
            GamePolicy {
                access_mode: Some(AccessMode::Gated),
                price: None,
            },
        );

        let kept = filter_accessible(
            &m,
            &member(BOB),
            vec![GAME.to_owned(), OTHER.to_owned(), "no-policy".to_owned()],
            |g| g.clone(),
        );
        assert_eq!(kept, vec![GAME.to_owned()]);
    }

    #[test]
    fn filtering_keeps_everything_for_the_admin() {
        let m = manifest(Some(AccessMode::Gated), &[]);
        let kept = filter_accessible(
            &m,
            &admin(),
            vec![GAME.to_owned(), "anything".to_owned()],
            |g| g.clone(),
        );
        assert_eq!(kept.len(), 2);
    }

    // --- transitions ------------------------------------------------------

    #[test]
    fn gated_to_free_grants_access_with_no_per_member_change() {
        let mut m = manifest(Some(AccessMode::Gated), &[]);
        assert!(!is_game_accessible(&m, &member(BOB), GAME));

        m.games.get_mut(GAME).unwrap().access_mode = Some(AccessMode::Free);
        assert!(is_game_accessible(&m, &member(BOB), GAME));
    }

    #[test]
    fn free_to_gated_falls_back_to_normal_gated_rules() {
        let mut m = manifest(Some(AccessMode::Free), &[(BOB, &[GAME])]);
        m.games.get_mut(GAME).unwrap().access_mode = Some(AccessMode::Gated);

        // Bob keeps access because he holds a grant...
        assert_eq!(
            decide(&m, &member(BOB), GAME),
            AccessDecision::AllowedByGrant
        );
        // ...but Marc, who never had one, does not.
        assert_eq!(
            decide(&m, &member(MARC), GAME),
            AccessDecision::DeniedGated
        );
    }

    #[test]
    fn all_to_gated_falls_back_to_normal_gated_rules() {
        let mut m = manifest(Some(AccessMode::All), &[]);
        assert!(is_game_accessible(&m, &member(BOB), GAME));

        m.games.get_mut(GAME).unwrap().access_mode = Some(AccessMode::Gated);
        assert_eq!(
            decide(&m, &member(BOB), GAME),
            AccessDecision::DeniedGated
        );
    }

    // --- price ------------------------------------------------------------

    #[test]
    fn a_null_price_is_not_the_same_as_free() {
        let m = manifest(Some(AccessMode::Gated), &[]);
        assert!(price_for(&m, GAME).is_none());
        // Still gated, still denied: no price does not mean no charge.
        assert_eq!(
            decide(&m, &member(BOB), GAME),
            AccessDecision::DeniedGated
        );
    }

    #[test]
    fn prices_render_from_minor_units_without_floating_point() {
        let cad = |n| Price {
            amount_minor: n,
            currency: "CAD".to_owned(),
        };
        assert_eq!(format_price(&cad(1299)), "$12.99 CAD");
        assert_eq!(format_price(&cad(1200)), "$12.00 CAD");
        assert_eq!(format_price(&cad(5)), "$0.05 CAD");
        assert_eq!(format_price(&cad(0)), "$0.00 CAD");
        // 0.1 + 0.2 problems cannot arise: the value never leaves integers.
        assert_eq!(format_price(&cad(3000)), "$30.00 CAD");
    }

    #[test]
    fn a_price_survives_a_json_round_trip_exactly() {
        let p = Price {
            amount_minor: 1299,
            currency: "CAD".to_owned(),
        };
        let json = serde_json::to_string(&p).expect("serialise");
        assert!(json.contains("1299"), "{json}");
        assert!(!json.contains("12.99"), "must not become a float: {json}");
        let back: Price = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, p);
    }
}
