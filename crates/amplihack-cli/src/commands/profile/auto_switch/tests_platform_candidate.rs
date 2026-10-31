//! Issue #1700: auto-switch and the API-key tier. Pure: profiles, readings,
//! the overflow order and the gate.
//!
//! Contract:
//! - `OverflowPool` gains `platform: Option<&ProfileName>`: the API-key
//!   profile, which `tiers()` also put last in `order`. Its block, when
//!   active, is in `blocked` like a spend block.
//! - The API-key profile is never probed: it has no reading, and that never
//!   makes it plan capacity (it is in `order`, so never in the plan pool).
//! - Onto it only as the last overflow entry, when the gate allows and no
//!   credit profile can be used.
//! - A session on it moves, ignoring holds, to a plan profile with capacity
//!   by the gate's test, else to the first usable credit profile; else it
//!   stays (`None`).
//! - A session on a credit profile never moves to the API key while it can
//!   stay (no spend block).
//!
//! Issue #1804:
//! - `OverflowInputs` takes its order from `overflow::tiers_detail`: an
//!   API-key profile that `overflow=` does not name is not in `order` and
//!   is not `platform`.
//! - Onto the API key only when no plan profile, the current one included,
//!   has room (`rotate::plan_room::has_room`: both windows below 100%). A
//!   missing reading counts as no room.
//! - Before journalling a move onto the API key, `swap.rs` runs the login
//!   check; on failure nothing is moved.

use super::candidate::{self, OverflowPool, Pool};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::Gate;
use std::collections::BTreeMap;

const NOW: i64 = 1_760_000_000;
const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn five_hour(five: f64, resets_in: i64) -> Reading {
    Reading {
        five_hour: Some(Window {
            used_percentage: five,
            resets_at: NOW + resets_in,
        }),
        seven_day: Some(Window {
            used_percentage: 10.0,
            resets_at: NOW + 86_400,
        }),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    }
}

/// Plan: `work`, `spare`. Credits: `cred`. API key: `console` (no reading).
struct Case {
    profiles: Vec<ProfileName>,
    readings: BTreeMap<String, Reading>,
    order: Vec<ProfileName>,
    blocked: Vec<String>,
    console: ProfileName,
}

impl Case {
    /// Every plan profile at 100%, resetting beyond the wait.
    fn all_plan_out() -> Self {
        let mut readings = BTreeMap::new();
        readings.insert("work".into(), five_hour(100.0, 4 * 3600));
        readings.insert("spare".into(), five_hour(100.0, 4 * 3600));
        readings.insert("cred".into(), five_hour(100.0, 4 * 3600));
        Self {
            profiles: ["work", "spare", "cred", "console"]
                .iter()
                .map(|p| n(p))
                .collect(),
            readings,
            order: vec![n("cred"), n("console")],
            blocked: Vec::new(),
            console: n("console"),
        }
    }

    fn with(mut self, name: &str, r: Reading) -> Self {
        self.readings.insert(name.into(), r);
        self
    }

    fn without(mut self, name: &str) -> Self {
        self.readings.remove(name);
        self
    }

    fn block(mut self, name: &str) -> Self {
        self.blocked.push(name.into());
        self
    }

    fn no_credits(mut self) -> Self {
        self.order.retain(|o| o.as_str() != "cred");
        self.profiles.retain(|o| o.as_str() != "cred");
        self
    }

    fn pick(&self, current: &str) -> Option<String> {
        self.pick_where(current, |_| true)
    }

    /// [`Self::pick`] with `ineligible` failing the caller's filter.
    fn pick_without(&self, current: &str, ineligible: &str) -> Option<String> {
        self.pick_where(current, |p| p.as_str() != ineligible)
    }

    fn pick_where(&self, current: &str, eligible: impl Fn(&ProfileName) -> bool) -> Option<String> {
        let exclude = Vec::new();
        let pool = Pool {
            profiles: &self.profiles,
            readings: &self.readings,
            threshold: 90,
            exclude: &exclude,
            now: NOW,
        };
        let ov = OverflowPool {
            order: &self.order,
            blocked: &self.blocked,
            gate: Some(GATE),
            platform: Some(&self.console),
        };
        candidate::select_with_overflow(&pool, &ov, current, eligible).map(|p| p.to_string())
    }
}

#[test]
fn credits_come_before_the_api_key() {
    assert_eq!(Case::all_plan_out().pick("work").as_deref(), Some("cred"));
}

#[test]
fn the_api_key_follows_a_blocked_credit_profile() {
    let c = Case::all_plan_out().block("cred");
    assert_eq!(c.pick("work").as_deref(), Some("console"));
    assert_eq!(
        Case::all_plan_out().no_credits().pick("work").as_deref(),
        Some("console")
    );
}

#[test]
fn a_blocked_api_key_is_never_chosen() {
    let c = Case::all_plan_out().block("cred").block("console");
    assert_ne!(c.pick("work").as_deref(), Some("console"));
}

#[test]
fn the_api_key_needs_the_gate() {
    // A plan profile resets within the wait.
    let c = Case::all_plan_out()
        .no_credits()
        .with("spare", five_hour(100.0, 600));
    assert_ne!(c.pick("work").as_deref(), Some("console"));
}

#[test]
fn a_plan_profile_with_capacity_wins_and_the_api_keys_missing_reading_is_not_capacity() {
    let c = Case::all_plan_out().with("spare", five_hour(40.0, 4 * 3600));
    assert_eq!(c.pick("work").as_deref(), Some("spare"));
    // No plan capacity: console's absent reading must not count as one,
    // so it is reached only through the overflow order, after credits.
    assert_eq!(Case::all_plan_out().pick("work").as_deref(), Some("cred"));
}

#[test]
fn a_session_on_the_api_key_moves_to_a_plan_profile_with_capacity() {
    let c = Case::all_plan_out().with("spare", five_hour(84.0, 4 * 3600));
    assert_eq!(c.pick("console").as_deref(), Some("spare"));
}

#[test]
fn a_session_on_the_api_key_moves_to_credits_when_no_plan_has_capacity() {
    assert_eq!(
        Case::all_plan_out().pick("console").as_deref(),
        Some("cred")
    );
}

#[test]
fn a_session_on_the_api_key_stays_when_nothing_else_can_be_used() {
    assert_eq!(Case::all_plan_out().block("cred").pick("console"), None);
    assert_eq!(Case::all_plan_out().no_credits().pick("console"), None);
}

#[test]
fn a_session_on_credits_never_moves_to_the_api_key_while_it_can_stay() {
    assert_eq!(Case::all_plan_out().pick("cred"), None);
}

#[test]
fn a_session_on_spend_blocked_credits_moves_to_the_api_key() {
    let c = Case::all_plan_out().block("cred");
    assert_eq!(c.pick("cred").as_deref(), Some("console"));
}

// --- Issue #1804: room and consent ------------------------------------------------------

#[test]
fn a_plan_profile_with_room_keeps_work_off_the_api_key() {
    // spare is past the threshold but below 100%: no capacity, but room.
    let c = Case::all_plan_out()
        .no_credits()
        .with("spare", five_hour(99.0, 4 * 3600));
    assert_ne!(c.pick("work").as_deref(), Some("console"));
}

#[test]
fn the_current_profile_with_room_keeps_work_off_the_api_key() {
    let c = Case::all_plan_out()
        .no_credits()
        .with("work", five_hour(95.0, 4 * 3600));
    assert_ne!(c.pick("work").as_deref(), Some("console"));
}

#[test]
fn a_spend_blocked_credit_session_does_not_move_to_the_api_key_while_a_plan_profile_has_room() {
    let c = Case::all_plan_out()
        .block("cred")
        .with("spare", five_hour(99.0, 4 * 3600));
    assert_ne!(c.pick("cred").as_deref(), Some("console"));
}

#[test]
fn a_missing_plan_reading_counts_as_no_room() {
    // An eligible profile with no reading is step 1's: it costs nothing. One
    // that cannot be used must not hold work off the API key as "room".
    let c = Case::all_plan_out().no_credits().without("spare");
    assert_eq!(c.pick("work").as_deref(), Some("spare"));
    assert_eq!(c.pick_without("work", "spare").as_deref(), Some("console"));
}

/// `root` with `work` (an OAuth profile) and `console` (an API-key
/// profile: `{}` login, no `oauthAccount`). No home is read.
fn root_with_console() -> (tempfile::TempDir, std::path::PathBuf) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = t.path().join("root");
    let store = crate::commands::profile::store::ProfileStore::new(root.clone());
    store.add(&n("work")).unwrap();
    store.add(&n("console")).unwrap();
    store.add(&n("cred")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let private = |p: std::path::PathBuf, body: &str| {
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    };
    private(
        root.join("work/.credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#,
    );
    private(
        root.join("cred/.credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#,
    );
    private(root.join("console/.credentials.json"), "{}");
    (t, root)
}

fn inputs(
    root: &std::path::Path,
    overflow: Vec<crate::commands::profile::overflow::OverflowEntry>,
) -> super::overflow_inputs::OverflowInputs {
    let cfg = super::config::Config {
        enabled: true,
        threshold: 90,
        exclude: Vec::new(),
        overflow,
        kind: Default::default(),
    };
    super::overflow_inputs::OverflowInputs::load_with_gate(
        root,
        &cfg,
        (
            Some(GATE),
            crate::commands::profile::rotate::GateSource::Environment,
        ),
        NOW,
    )
}

#[test]
fn order_of_leaves_out_an_api_key_profile_that_is_not_marked() {
    use crate::commands::profile::overflow::OverflowEntry;
    let (_t, root) = root_with_console();
    let i = inputs(&root, vec![OverflowEntry::new(n("cred"), 1)]);
    assert_eq!(i.order, vec![n("cred")], "no implicit API-key entry");
    assert_eq!(i.platform, None);
    let none = inputs(&root, Vec::new());
    assert!(none.order.is_empty(), "{:?}", none.order);
    assert_eq!(none.platform, None);
}

#[test]
fn order_of_puts_a_marked_api_key_profile_last() {
    use crate::commands::profile::overflow::OverflowEntry;
    let (_t, root) = root_with_console();
    let i = inputs(
        &root,
        vec![
            OverflowEntry::new(n("console"), 1),
            OverflowEntry::new(n("cred"), 1),
        ],
    );
    assert_eq!(i.order, vec![n("cred"), n("console")]);
    assert_eq!(i.platform, Some(n("console")));
}
