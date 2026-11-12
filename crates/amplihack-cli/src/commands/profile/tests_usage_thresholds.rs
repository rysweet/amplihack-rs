//! Issue #2007: the thresholds line and object of `profile usage`
//! (`usage_thresholds.rs`). The contract is docs/reference/profile-command.md,
//! "Thresholds in force" and "Summary JSON".
//!
//! The seam:
//!
//! - `resolve(rotator: Option<Gate>, env, cfg) -> Thresholds`: each value
//!   from `rotate::settings::thresholds(env, cfg)`, then the running
//!   rotator's recorded `session_at` and `week_at` over it with origin
//!   `rotator`, even when the config value for that key is bad;
//! - `text_line(&Thresholds) -> String`: the one ASCII line, without its
//!   newline, every message cleaned like table names;
//! - `json(&Thresholds) -> serde_json::Value`: the `thresholds` object of
//!   `--summary-json`, with messages uncleaned.
//!
//! `env` and `cfg` are injected; nothing here reads the process
//! environment, a real HOME or a rotator's files.

use super::overflow::Gate;
use super::rotate::settings::no_config;
use super::usage_thresholds::{json, resolve, text_line};
use std::collections::HashMap;
use std::ffi::OsString;

type CfgResult = Result<Option<u64>, String>;

fn no_env(_: &str) -> Option<OsString> {
    None
}

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |k| map.get(k).cloned()
}

fn cfg_of(pairs: &[(&str, u64)]) -> impl Fn(&str) -> CfgResult + use<> {
    let map: HashMap<String, u64> = pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    move |k| Ok(map.get(k).copied())
}

fn cfg_bad(bad: &'static str, why: &'static str) -> impl Fn(&str) -> CfgResult {
    move |k| {
        if k == bad {
            Err(why.to_string())
        } else {
            Ok(None)
        }
    }
}

fn gate(session_at: u8, week_at: u8) -> Gate {
    Gate {
        wait: 1800,
        unknown_reset: false,
        session_at,
        week_at,
    }
}

const DEFAULT_LINE: &str = "Rotator thresholds: session-at 85% (default) · week-at 90% (default, review reserve 10%) · interval 600s (default) · codex credit floor 3000 (default)";

fn fix_msg(key: &str, why: &str) -> String {
    format!(
        "invalid rotate setting {key}: {why}; fix it with `amplihack config set {key} <value>` or `amplihack config unset {key}`"
    )
}

// --- The text line ---------------------------------------------------------

#[test]
fn with_nothing_set_the_line_shows_the_defaults() {
    let t = resolve(None, &no_env, &no_config);
    assert_eq!(text_line(&t), DEFAULT_LINE);
}

#[test]
fn the_documented_override_example() {
    // `config set rotator.week_at 80` and `AMPLIHACK_ROTATE_INTERVAL=300`.
    let t = resolve(
        None,
        &env_of(&[("AMPLIHACK_ROTATE_INTERVAL", "300")]),
        &cfg_of(&[("rotator.week_at", 80)]),
    );
    assert_eq!(
        text_line(&t),
        "Rotator thresholds: session-at 85% (default) · week-at 80% (config, review reserve 20%) · interval 300s (env) · codex credit floor 3000 (default)"
    );
}

#[test]
fn every_key_from_config() {
    let t = resolve(
        None,
        &no_env,
        &cfg_of(&[
            ("rotator.session_at", 70),
            ("rotator.week_at", 100),
            ("rotator.interval", 120),
            ("rotator.codex_credit_floor", 0),
        ]),
    );
    assert_eq!(
        text_line(&t),
        "Rotator thresholds: session-at 70% (config) · week-at 100% (config, review reserve 0%) · interval 120s (config) · codex credit floor 0 (config)"
    );
}

#[test]
fn a_running_rotator_s_session_and_week_points_win_with_origin_rotator() {
    let t = resolve(
        Some(gate(70, 95)),
        &env_of(&[("AMPLIHACK_ROTATE_SESSION_AT", "60")]),
        &cfg_of(&[("rotator.session_at", 50), ("rotator.interval", 120)]),
    );
    assert_eq!(
        text_line(&t),
        "Rotator thresholds: session-at 70% (rotator) · week-at 95% (rotator, review reserve 5%) · interval 120s (config) · codex credit floor 3000 (default)"
    );
}

#[test]
fn a_running_rotator_hides_a_bad_config_value_for_its_own_keys_only() {
    let t = resolve(
        Some(gate(70, 90)),
        &no_env,
        &cfg_bad("rotator.session_at", "must be an integer from 1 to 100"),
    );
    let line = text_line(&t);
    assert!(
        line.starts_with("Rotator thresholds: session-at 70% (rotator) · "),
        "{line}"
    );
    let t = resolve(
        Some(gate(70, 90)),
        &no_env,
        &cfg_bad("rotator.interval", "must be an integer from 1 to 86400"),
    );
    assert!(
        text_line(&t).contains("interval invalid ("),
        "{}",
        text_line(&t)
    );
}

#[test]
fn a_bad_key_shows_its_message_and_leaves_the_others() {
    let t = resolve(
        None,
        &no_env,
        &cfg_bad("rotator.session_at", "must be an integer from 1 to 100"),
    );
    assert_eq!(
        text_line(&t),
        format!(
            "Rotator thresholds: session-at invalid ({}) · week-at 90% (default, review reserve 10%) · interval 600s (default) · codex credit floor 3000 (default)",
            fix_msg("rotator.session_at", "must be an integer from 1 to 100")
        )
    );
}

#[test]
fn a_bad_week_at_has_no_review_reserve_on_the_line() {
    let t = resolve(
        None,
        &no_env,
        &cfg_bad("rotator.week_at", "must be an integer from 1 to 100"),
    );
    let line = text_line(&t);
    assert!(
        line.contains(&format!(
            " · week-at invalid ({}) · interval 600s (default)",
            fix_msg("rotator.week_at", "must be an integer from 1 to 100")
        )),
        "{line}"
    );
    assert!(!line.contains("review reserve"), "{line}");
}

#[test]
fn a_bad_codex_floor_is_named_on_the_line() {
    let t = resolve(
        None,
        &no_env,
        &cfg_bad(
            "rotator.codex_credit_floor",
            "must be an integer from 0 to 100000000",
        ),
    );
    assert!(
        text_line(&t).ends_with(&format!(
            " · codex credit floor invalid ({})",
            fix_msg(
                "rotator.codex_credit_floor",
                "must be an integer from 0 to 100000000"
            )
        )),
        "{}",
        text_line(&t)
    );
}

#[test]
fn a_bad_variable_is_shown_not_replaced_by_config() {
    let t = resolve(
        None,
        &env_of(&[("AMPLIHACK_ROTATE_INTERVAL", "soon")]),
        &cfg_of(&[("rotator.interval", 120)]),
    );
    let line = text_line(&t);
    assert!(line.contains("interval invalid ("), "{line}");
    assert!(line.contains("AMPLIHACK_ROTATE_INTERVAL"), "{line}");
    assert!(!line.contains("120s"), "{line}");
}

#[test]
fn messages_are_cleaned_and_the_line_is_ascii() {
    let t = resolve(
        None,
        &no_env,
        &cfg_bad("rotator.week_at", "evil\u{1b}[2J\u{202e}\u{7}"),
    );
    let line = text_line(&t);
    for c in ['\u{1b}', '\u{202e}', '\u{7}', '\n'] {
        assert!(!line.contains(c), "{c:?} in {line:?}");
    }
    assert!(line.replace(" · ", " ").is_ascii(), "{line:?}");
    assert!(line.contains("rotator.week_at"), "{line}");
}

// --- The JSON object -------------------------------------------------------

#[test]
fn the_default_object_is_the_documented_one() {
    let t = resolve(None, &no_env, &no_config);
    assert_eq!(
        json(&t),
        serde_json::json!({
            "session_at": { "value": 85, "origin": "default" },
            "week_at": { "value": 90, "origin": "default" },
            "review_reserve": 10,
            "interval": { "value": 600, "origin": "default" },
            "codex_credit_floor": { "value": 3000, "origin": "default" }
        })
    );
}

#[test]
fn values_are_integers_and_origins_are_lowercase() {
    let t = resolve(
        Some(gate(70, 80)),
        &env_of(&[("AMPLIHACK_ROTATE_INTERVAL", "300")]),
        &cfg_of(&[("rotator.codex_credit_floor", 100_000_000)]),
    );
    let v = json(&t);
    assert_eq!(
        v,
        serde_json::json!({
            "session_at": { "value": 70, "origin": "rotator" },
            "week_at": { "value": 80, "origin": "rotator" },
            "review_reserve": 20,
            "interval": { "value": 300, "origin": "env" },
            "codex_credit_floor": { "value": 100_000_000, "origin": "config" }
        })
    );
    for k in ["session_at", "week_at", "interval", "codex_credit_floor"] {
        assert!(v[k]["value"].is_u64(), "{k}: {}", v[k]);
    }
}

#[test]
fn a_bad_key_is_null_with_the_raw_error() {
    let raw = "evil\u{1b}[2J\u{202e}";
    let t = resolve(None, &no_env, &cfg_bad("rotator.week_at", raw));
    let v = json(&t);
    let w = &v["week_at"];
    assert!(w["value"].is_null(), "{w}");
    assert!(w["origin"].is_null(), "{w}");
    assert!(v["review_reserve"].is_null(), "{v}");
    assert!(
        v.as_object().unwrap().contains_key("review_reserve"),
        "review_reserve is always present at the top level: {v}"
    );
    assert!(w.get("review_reserve").is_none(), "not inside week_at: {w}");
    assert_eq!(w["error"], fix_msg("rotator.week_at", raw).as_str());
    // JSON escaping, not display cleaning: the raw control survives a parse.
    let text = serde_json::to_string(&v).unwrap();
    let back: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        back["week_at"]["error"]
            .as_str()
            .unwrap()
            .contains('\u{1b}')
    );
    assert!(!text.contains('\u{1b}'), "escaped in the text: {text:?}");
    // The other keys have no `error`.
    for k in ["session_at", "interval", "codex_credit_floor"] {
        assert!(v[k].get("error").is_none(), "{k}: {}", v[k]);
    }
}

#[test]
fn the_object_always_has_all_four_keys_and_the_review_reserve() {
    for bad in [
        "rotator.session_at",
        "rotator.week_at",
        "rotator.interval",
        "rotator.codex_credit_floor",
    ] {
        let v = json(&resolve(None, &no_env, &cfg_bad(bad, "x")));
        let mut k: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        k.sort_unstable();
        assert_eq!(
            k,
            [
                "codex_credit_floor",
                "interval",
                "review_reserve",
                "session_at",
                "week_at"
            ],
            "{bad}"
        );
    }
}

// --- Whole-file problems (issue #2007, docs/reference/user-config.md) -------

#[cfg(unix)]
fn group_writable_home(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap().join("home");
    let dir = home.join(".amplihack");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("config.toml");
    std::fs::write(&file, body).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664)).unwrap();
    (tmp, home)
}

#[cfg(unix)]
#[test]
fn a_group_writable_file_shows_the_whole_file_fix_and_no_path() {
    use super::rotate::settings::UserConfig;
    let (_tmp, home) = group_writable_home("[rotator]\nweek_at = 80\n");
    let cfg = UserConfig::at(&home);
    let t = resolve(None, &no_env, &|k| cfg.get(k));
    let v = json(&t);
    let want = "invalid rotate setting rotator.week_at: ignored because the file is writable by group or others; fix the user config file first (`amplihack config list --show-origin` shows its path and the problem)";
    assert_eq!(v["week_at"]["error"], want, "{}", v["week_at"]);
    assert!(v["week_at"]["value"].is_null());
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains(&home.display().to_string()), "{text}");
    assert!(!text.contains("amplihack config set"), "{text}");
    let line = text_line(&t);
    assert!(!line.contains(&home.display().to_string()), "{line}");
    assert!(line.contains("fix the user config file first"), "{line}");
    assert!(line.replace(" · ", " ").is_ascii(), "{line}");
}

#[cfg(unix)]
#[test]
fn a_running_rotator_still_wins_over_a_group_writable_file() {
    use super::rotate::settings::UserConfig;
    let (_tmp, home) = group_writable_home("[rotator]\nweek_at = 80\n");
    let cfg = UserConfig::at(&home);
    let t = resolve(Some(gate(70, 80)), &no_env, &|k| cfg.get(k));
    let v = json(&t);
    assert_eq!(v["session_at"]["value"], 70);
    assert_eq!(v["session_at"]["origin"], "rotator");
    assert_eq!(v["week_at"]["value"], 80);
    assert!(v["week_at"].get("error").is_none(), "{}", v["week_at"]);
    // The keys the rotator does not record still show the problem.
    assert!(v["interval"]["value"].is_null(), "{}", v["interval"]);
    assert!(
        v["interval"]["error"]
            .as_str()
            .unwrap()
            .ends_with("shows its path and the problem)"),
        "{}",
        v["interval"]
    );
}

#[test]
fn a_running_rotator_never_changes_the_interval_or_floor_origin() {
    // The gate record holds no interval and no floor: both keep the origin
    // a rotator started now would use (design finding 1).
    let t = resolve(Some(gate(70, 80)), &no_env, &no_config);
    let i = t.interval.as_ref().unwrap();
    assert_eq!((i.value, i.origin.label()), (600, "default"));
    let c = t.codex_credit_floor.as_ref().unwrap();
    assert_eq!((c.value, c.origin.label()), (3000, "default"));
    assert_eq!(t.session_at.as_ref().unwrap().origin.label(), "rotator");
    assert_eq!(t.week_at.as_ref().unwrap().origin.label(), "rotator");

    let t = resolve(
        Some(gate(70, 80)),
        &env_of(&[("AMPLIHACK_ROTATE_INTERVAL", "300")]),
        &cfg_of(&[
            ("rotator.interval", 120),
            ("rotator.codex_credit_floor", 10_000),
        ]),
    );
    let o = json(&t);
    assert_eq!(o["interval"]["value"], 300);
    assert_eq!(o["interval"]["origin"], "env");
    assert_eq!(o["codex_credit_floor"]["value"], 10_000);
    assert_eq!(o["codex_credit_floor"]["origin"], "config");
    assert_eq!(o["session_at"]["origin"], "rotator");
}
