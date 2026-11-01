//! Issue #1797: capacity weights (`<root>/.auto-switch.d/capacity-weights.json`)
//! and the usable-now session threshold.
//!
//! Contract (docs/reference/profile-command.md, "Capacity weights" and
//! "Usable now"):
//! - No file: every weight is 1.0, silently.
//! - A symlink, a file over 64 KiB, one writable by group or others, invalid
//!   JSON or a top level that is not an object: every weight is 1.0 and one
//!   fixed warning, never a path.
//! - A value that is not a number in (0, 1000]: that weight is 1.0, with one
//!   warning naming the profile (cleaned for the terminal).
//! - Names that match no profile are ignored.
//! - Threshold: the flag, else `AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT`
//!   (1 to 100; empty is unset; anything else warns and uses 95), else 95.

use super::capacity_weight::{ENV, FILE, Weights, parse, usable_threshold};
use std::ffi::{OsStr, OsString};

const FILE_WARNING_START: &str = "warning: capacity-weights.json ignored (";
const FILE_WARNING_END: &str = "); every weight is 1.0";

fn value_warning(name: &str) -> String {
    format!(
        "warning: capacity-weights.json: weight for \"{name}\" ignored \
         (use a number above 0 and at most 1000); using 1.0"
    )
}

fn assert_file_warning(w: &[String]) {
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(
        w[0].starts_with(FILE_WARNING_START) && w[0].ends_with(FILE_WARNING_END),
        "{}",
        w[0]
    );
    assert!(!w[0].contains('/'), "never a path: {}", w[0]);
}

fn assert_uniform(w: &Weights) {
    for n in ["team", "work", "anything"] {
        assert_eq!(w.of(n), 1.0, "{n}");
    }
    assert!(!w.any_differs_among(["team", "work", "anything"]));
}

#[test]
fn the_names_are_the_documented_ones() {
    assert_eq!(FILE, "capacity-weights.json");
    assert_eq!(ENV, "AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT");
}

#[test]
fn default_weights_are_all_one() {
    assert_uniform(&Weights::default());
}

// ---------------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------------

#[test]
fn a_valid_object_sets_each_weight_and_others_stay_one() {
    let (w, warnings) = parse(br#"{ "team": 2.0, "scratch": 0.5, "max": 1000 }"#);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(w.of("team"), 2.0);
    assert_eq!(w.of("scratch"), 0.5);
    assert_eq!(w.of("max"), 1000.0);
    assert_eq!(w.of("work"), 1.0, "a profile not in the file");
    assert!(w.any_differs_among(["work", "scratch"]));
    assert!(
        !w.any_differs_among(["work", "other"]),
        "only the named profiles count"
    );
}

#[test]
fn weights_of_exactly_one_do_not_differ() {
    let (w, warnings) = parse(br#"{"team": 1.0, "work": 1}"#);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!w.any_differs_among(["team", "work"]));
}

#[test]
fn an_empty_object_is_valid() {
    let (w, warnings) = parse(b"{}");
    assert!(warnings.is_empty());
    assert_uniform(&w);
}

#[test]
fn invalid_json_gives_one_warning_and_every_weight_one() {
    for bytes in [&b"{ team: 2 }"[..], b"", b"\xff\xfe", b"{\"team\": 2.0"] {
        let (w, warnings) = parse(bytes);
        assert_file_warning(&warnings);
        assert_uniform(&w);
    }
}

#[test]
fn a_top_level_that_is_not_an_object_gives_one_warning() {
    for bytes in [&b"[1, 2]"[..], b"2.0", b"\"team\"", b"null"] {
        let (w, warnings) = parse(bytes);
        assert_file_warning(&warnings);
        assert_uniform(&w);
    }
}

#[test]
fn each_bad_value_warns_once_naming_the_profile_and_reads_one() {
    let (w, warnings) = parse(
        br#"{"neg": -1, "zero": 0, "text": "NaN", "huge": 1e308, "over": 1000.0001,
            "nul": null, "yes": true, "list": [2], "good": 3}"#,
    );
    for n in ["neg", "zero", "text", "huge", "over", "nul", "yes", "list"] {
        assert_eq!(w.of(n), 1.0, "{n}");
        assert!(warnings.contains(&value_warning(n)), "{n}: {warnings:?}");
    }
    assert_eq!(warnings.len(), 8, "{warnings:?}");
    assert_eq!(w.of("good"), 3.0, "a good value next to bad ones is kept");
}

#[test]
fn a_tiny_positive_weight_is_accepted() {
    let (w, warnings) = parse(br#"{"tiny": 0.001}"#);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(w.of("tiny"), 0.001);
}

#[test]
fn a_hostile_name_in_a_warning_is_cleaned_and_cut() {
    let long = "x".repeat(10_000);
    let body = format!(r#"{{"\u001b[31mred": -1, "{long}": -1}}"#);
    let (_, warnings) = parse(body.as_bytes());
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for w in &warnings {
        assert!(!w.chars().any(char::is_control), "{w:?}");
        assert!(w.len() < 300, "cut: {} bytes", w.len());
    }
}

#[test]
fn parse_never_echoes_the_file_body() {
    let (_, warnings) = parse(b"{ secret-looking-garbage }");
    assert!(
        !warnings[0].contains("secret-looking-garbage"),
        "{warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// load (the file under <root>/.auto-switch.d)
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod load {
    use super::super::capacity_weight::{FILE, load};
    use super::{assert_file_warning, assert_uniform};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn root() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let state = t.path().join(".auto-switch.d");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        (t, state)
    }

    fn write(path: &Path, body: &[u8], mode: u32) {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn no_file_or_no_state_directory_is_silent() {
        let (t, _) = root();
        let (w, warnings) = load(t.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_uniform(&w);
        let bare = tempfile::tempdir().unwrap();
        let (w, warnings) = load(bare.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_uniform(&w);
        assert!(
            !bare.path().join(".auto-switch.d").exists(),
            "reading weights never creates the state directory"
        );
    }

    #[test]
    fn a_private_file_is_read() {
        let (t, state) = root();
        write(&state.join(FILE), br#"{"team": 2.5}"#, 0o600);
        let (w, warnings) = load(t.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(w.of("team"), 2.5);
    }

    #[test]
    fn a_file_readable_but_not_writable_by_others_is_read() {
        let (t, state) = root();
        write(&state.join(FILE), br#"{"team": 2.5}"#, 0o644);
        let (w, warnings) = load(t.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(w.of("team"), 2.5);
    }

    #[test]
    fn a_file_writable_by_group_or_others_is_refused() {
        for mode in [0o620, 0o602, 0o666] {
            let (t, state) = root();
            write(&state.join(FILE), br#"{"team": 2.5}"#, mode);
            let (w, warnings) = load(t.path());
            assert_file_warning(&warnings);
            assert_uniform(&w);
        }
    }

    #[test]
    fn a_root_writable_by_other_users_is_refused() {
        // Group write is allowed for the user's own private group, as for
        // the usage history; other-write never is.
        for mode in [0o707, 0o777] {
            let (t, state) = root();
            write(&state.join(FILE), br#"{"team": 2.5}"#, 0o600);
            fs::set_permissions(t.path(), fs::Permissions::from_mode(mode)).unwrap();
            let (w, warnings) = load(t.path());
            assert_file_warning(&warnings);
            assert_uniform(&w);
        }
    }

    #[test]
    fn a_missing_root_is_silent() {
        let t = tempfile::tempdir().unwrap();
        let (w, warnings) = load(&t.path().join("absent"));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_uniform(&w);
    }

    #[test]
    fn a_symlink_is_refused() {
        let (t, state) = root();
        let real = t.path().join("elsewhere.json");
        write(&real, br#"{"team": 2.5}"#, 0o600);
        std::os::unix::fs::symlink(&real, state.join(FILE)).unwrap();
        let (w, warnings) = load(t.path());
        assert_file_warning(&warnings);
        assert_uniform(&w);
    }

    #[test]
    fn a_file_over_64_kib_is_refused() {
        let (t, state) = root();
        let mut body = br#"{"team": 2.5, "pad": ""#.to_vec();
        body.extend(std::iter::repeat_n(b'x', 64 * 1024));
        body.extend(br#""}"#);
        write(&state.join(FILE), &body, 0o600);
        let (w, warnings) = load(t.path());
        assert_file_warning(&warnings);
        assert_uniform(&w);
    }

    #[test]
    fn a_directory_in_place_of_the_file_is_refused() {
        let (t, state) = root();
        fs::create_dir(state.join(FILE)).unwrap();
        let (w, warnings) = load(t.path());
        assert_file_warning(&warnings);
        assert_uniform(&w);
    }

    #[test]
    fn loading_never_writes_the_file() {
        let (t, state) = root();
        let body = br#"{"team": -4}"#;
        write(&state.join(FILE), body, 0o600);
        let _ = load(t.path());
        assert_eq!(fs::read(state.join(FILE)).unwrap(), body);
    }
}

// ---------------------------------------------------------------------------
// The usable-now threshold
// ---------------------------------------------------------------------------

fn env_warning(shown: &str) -> String {
    format!("warning: ignored {ENV} \"{shown}\" (use 1 to 100); using 95")
}

fn os(s: &str) -> OsString {
    OsString::from(s)
}

#[test]
fn the_default_is_95_with_no_flag_and_no_variable() {
    assert_eq!(usable_threshold(None, None), (95, None));
}

#[test]
fn the_variable_sets_the_threshold_from_1_to_100() {
    for (v, want) in [("1", 1), ("50", 50), ("100", 100)] {
        assert_eq!(usable_threshold(None, Some(&os(v))), (want, None), "{v}");
    }
}

#[test]
fn the_flag_wins_over_the_variable() {
    assert_eq!(usable_threshold(Some(80), Some(&os("50"))).0, 80);
    assert_eq!(usable_threshold(Some(100), None), (100, None));
}

#[test]
fn an_empty_variable_counts_as_unset() {
    assert_eq!(usable_threshold(None, Some(&os(""))), (95, None));
}

#[test]
fn a_bad_variable_warns_with_the_value_and_uses_95() {
    for v in ["0", "101", "most", "9.5", "-1", " 50", "50%", "1e2"] {
        let (pct, warning) = usable_threshold(None, Some(&os(v)));
        assert_eq!(pct, 95, "{v}");
        assert_eq!(warning.as_deref(), Some(env_warning(v).as_str()), "{v}");
    }
}

#[test]
fn a_bad_variable_is_shown_escaped_and_cut_to_32_characters() {
    let (pct, warning) = usable_threshold(None, Some(&os("\u{1b}[2J9")));
    assert_eq!(pct, 95);
    let w = warning.unwrap();
    assert!(!w.chars().any(char::is_control), "{w:?}");

    let (_, warning) = usable_threshold(None, Some(&os(&"7".repeat(200))));
    let w = warning.unwrap();
    let shown = w
        .split('"')
        .nth(1)
        .unwrap_or_else(|| panic!("quoted value: {w}"));
    assert!(shown.chars().count() <= 32, "{shown}");
}

#[cfg(unix)]
#[test]
fn a_variable_that_is_not_utf8_warns_and_uses_95() {
    use std::os::unix::ffi::OsStrExt;
    let (pct, warning) = usable_threshold(None, Some(OsStr::from_bytes(b"5\xff")));
    assert_eq!(pct, 95);
    let w = warning.expect("a warning");
    assert!(w.starts_with(&format!("warning: ignored {ENV} \"")), "{w}");
    assert!(w.ends_with("(use 1 to 100); using 95"), "{w}");
}
