//! Issue #1700: API-key (Claude Platform) profiles in the shared, read-only
//! rotator view. The CLI and the limit-resume hook both read `.auto-switch`
//! through `parse_toggle`, and the hook reads the rotate ledger loosely
//! through `parse_cycle`; both must understand the two additions.
//!
//! Contract:
//! - `Toggle.kind: Vec<(String, String)>`: the `kind=` line, as
//!   `(profile, kind)` in file order. Each entry is `<name>:<kind>`; kind is
//!   `platform`, `api-key` (stored as `platform`) or `subscription`. An
//!   entry with any other kind is left out (the CLI warns); the rest of the
//!   file is still read. A name twice, an invalid name or an entry without
//!   `:` makes the file corrupt, as does a second `kind=` line. An empty
//!   `kind=` is no overrides. The key is optional.
//! - `CycleStatus.platform_live: bool`: the ledger's `live_kind` is the
//!   string `"platform"` and `live` is a valid name. Any other value, or a
//!   wrongly typed one, reads as `false`; the ledger is still read.
//!   `has_capacity` and `on_overflow` do not change.

#![cfg(unix)]

use amplihack_utils::rotator_status::{parse_cycle, parse_toggle};

const NOW: i64 = 1_792_592_000;

fn kinds(raw: &str) -> Vec<(String, String)> {
    parse_toggle(raw.as_bytes()).expect("valid toggle").kind
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

// --- kind= ------------------------------------------------------------------

#[test]
fn a_toggle_without_kind_has_no_overrides() {
    assert!(kinds("enabled=off\nthreshold=90\nexclude=\noverflow=extra\n").is_empty());
    assert!(kinds("enabled=off\nkind=\n").is_empty(), "empty line");
}

#[test]
fn kind_entries_are_read_in_file_order_with_api_key_stored_as_platform() {
    assert_eq!(
        kinds(
            "enabled=on\noverflow=extra\nkind=console:platform,old-team:subscription,lab:api-key\n"
        ),
        vec![
            pair("console", "platform"),
            pair("old-team", "subscription"),
            pair("lab", "platform"),
        ]
    );
}

#[test]
fn an_unknown_kind_skips_only_that_entry() {
    assert_eq!(
        kinds("enabled=off\nkind=a:platform,b:enterprise,c:subscription\n"),
        vec![pair("a", "platform"), pair("c", "subscription")]
    );
}

#[test]
fn malformed_kind_lines_make_the_file_corrupt() {
    for bad in [
        "enabled=off\nkind=a:platform,a:subscription\n", // the same name twice
        "enabled=off\nkind=a:platform\nkind=b:platform\n", // the key twice
        "enabled=off\nkind=../x:platform\n",             // invalid name
        "enabled=off\nkind=console\n",                   // no kind
        "enabled=off\nkind=:platform\n",                 // no name
    ] {
        assert!(parse_toggle(bad.as_bytes()).is_err(), "{bad:?}");
    }
}

#[test]
fn a_kind_error_never_quotes_the_input() {
    let raw = b"enabled=off\nkind=CANARY-LEAK-PROBE:platform,CANARY-LEAK-PROBE:platform\n";
    let err = parse_toggle(raw).unwrap_err();
    assert!(!err.contains("CANARY"), "{err}");
}

#[test]
fn the_kind_line_does_not_disturb_the_other_keys() {
    let t = parse_toggle(
        b"enabled=on\nthreshold=75\nexclude=work\noverflow=extra:2\nkind=console:platform\n",
    )
    .unwrap();
    assert!(t.enabled);
    assert_eq!(t.threshold, 75);
    assert_eq!(t.exclude, vec!["work".to_string()]);
    assert_eq!(t.overflow, vec![("extra".to_string(), 2)]);
    assert_eq!(t.kind, vec![pair("console", "platform")]);
}

// --- live_kind in the ledger --------------------------------------------------

fn ledger(live_kind: &str) -> Vec<u8> {
    format!(
        r#"{{"version":1,"live":"console","since":{s},{live_kind}
            "last_cycle":{{"at":{t},"outcome":"stayed","overflow":"on"}}}}"#,
        s = NOW - 600,
        t = NOW - 10,
    )
    .into_bytes()
}

#[test]
fn platform_live_is_read_from_live_kind() {
    let s = parse_cycle(&ledger(r#""live_kind":"platform","#), NOW).expect("valid");
    assert!(s.platform_live);
    assert_eq!(s.live.as_deref(), Some("console"));
}

#[test]
fn any_other_live_kind_reads_as_not_platform() {
    for lk in [
        "",
        r#""live_kind":"subscription","#,
        r#""live_kind":"Platform","#,
        r#""live_kind":1,"#,
        r#""live_kind":null,"#,
        r#""live_kind":["platform"],"#,
    ] {
        let s = parse_cycle(&ledger(lk), NOW).unwrap_or_else(|| panic!("still read: {lk}"));
        assert!(!s.platform_live, "{lk}");
    }
}

#[test]
fn platform_live_needs_a_valid_live_name() {
    let raw = format!(
        r#"{{"version":1,"live":"../x","live_kind":"platform",
            "last_cycle":{{"at":{t},"outcome":"stayed"}}}}"#,
        t = NOW - 10
    );
    let s = parse_cycle(raw.as_bytes(), NOW).unwrap();
    assert!(!s.platform_live);
}

#[test]
fn a_platform_block_in_the_ledger_does_not_change_capacity_or_overflow() {
    let raw = format!(
        r#"{{"version":1,"live":"console","live_kind":"platform",
            "platform_block":{{"at":{a},"until":{u},"reason":"key_exhausted"}},
            "last_cycle":{{"at":{t},"outcome":"stayed","overflow":"on"}}}}"#,
        a = NOW - 100,
        u = NOW + 86_400,
        t = NOW - 10
    );
    let s = parse_cycle(raw.as_bytes(), NOW).expect("unknown fields are ignored");
    assert!(s.has_capacity(), "stayed, no alert");
    assert!(s.on_overflow(), "overflow=on");
    assert!(s.platform_live);
}
