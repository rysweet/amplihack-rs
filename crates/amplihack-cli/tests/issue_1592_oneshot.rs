//! Issue #1592: `amplihack profile rotate` once, end to end, against a
//! temporary HOME, fake profiles and a stub `claude`. Logins are checked by
//! inode only. Linux-only: the swap needs `renameat2(RENAME_EXCHANGE)` and
//! candidate checks need `/proc`.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

fn lines(out: &std::process::Output) -> Vec<String> {
    text(&out.stdout).lines().map(str::to_string).collect()
}

/// Paths, inodes, sizes, times and (for anything but a login) contents of
/// the profile root, `~/.claude` and `~/.claude.json`.
fn snapshot(f: &Fixture) -> BTreeMap<String, String> {
    fn walk(base: &Path, p: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(m) = fs::symlink_metadata(p) else {
            return;
        };
        let rel = p.strip_prefix(base).unwrap().display().to_string();
        let body = if m.is_file() && !p.ends_with(CREDS) {
            fs::read(p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.insert(
            rel,
            format!(
                "{:o} {} {} {} {body}",
                m.mode(),
                m.ino(),
                m.len(),
                m.mtime_nsec()
            ),
        );
        if m.is_dir() {
            for e in fs::read_dir(p).unwrap() {
                walk(base, &e.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    for p in [
        f.root.clone(),
        f.home.join(".claude"),
        f.home.join(".claude.json"),
    ] {
        walk(&f.home, &p, &mut out);
    }
    out
}

#[test]
fn a_threshold_swap_moves_both_logins_and_only_the_account() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "rotate"]);
    let ls = lines(&out);
    for l in &ls {
        assert_log_grammar(l, true);
    }
    assert!(
        ls.iter().any(|l| l
            .ends_with("swap work->spare live s=90% w=40% target s=10% w=20% reason=threshold")),
        "{ls:?}"
    );
    assert!(
        ls.iter()
            .any(|l| l
                .contains("cycle live=work work:s=90%,w=40% extra:s=50%,w=50% spare:s=10%,w=20%")),
        "{ls:?}"
    );

    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    assert_eq!(ino(&f.slot("spare")), None);
    assert_eq!(ino(&f.slot("extra")), Some(f.extra_ino));
    f.assert_each_login_once();

    assert_eq!(f.home_account(), "uuid-spare");
    let home: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(home["primaryApiKey"], "SECRET-KEY-HOME");
    assert_eq!(home["numStartups"], 7);

    let ledger = f.ledger().expect("ledger written");
    assert_eq!(ledger["live"], "spare");
    assert_eq!(ledger["previous"], "work");
    assert!(!f.rotate_dir().join("journal.json").exists());

    let log = f.log();
    for l in &ls {
        assert!(log.contains(l.as_str()), "stdout line not in the log: {l}");
    }
}

#[test]
fn an_upgrade_swap_needs_thirty_points() {
    let f = Fixture::new();
    f.usage("work", 50, 0);
    f.usage("spare", 25, 0);
    f.usage("extra", 30, 0);
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        !text(&out.stdout).contains("swap "),
        "25 points is not enough"
    );
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));

    f.usage("spare", 20, 0);
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        text(&out.stdout)
            .contains("swap work->spare live s=50% w=0% target s=20% w=0% reason=upgrade"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
}

#[test]
fn a_cycle_that_stays_moves_nothing_and_records_the_cycle() {
    let f = Fixture::new();
    f.usage("work", 20, 20);
    let out = f.ok(&["profile", "rotate"]);
    let stdout = text(&out.stdout);
    assert!(!stdout.contains("swap "), "{stdout}");
    assert!(stdout.contains("cycle live=work"), "{stdout}");
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    f.assert_each_login_once();
    let ledger = f.ledger().expect("last_cycle recorded");
    assert_eq!(ledger["last_cycle"]["outcome"], "stayed");
}

#[test]
fn the_hold_blocks_an_upgrade_back_and_hold_zero_allows_it() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    // spare now has 40 headroom, work (reset) 100: an upgrade by 60.
    f.usage("spare", 60, 0);
    f.usage("work", 0, 0);
    f.usage("extra", 99, 99);
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        !text(&out.stdout).contains("swap "),
        "held: {}",
        text(&out.stdout)
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));

    let out = f.ok(&["profile", "rotate", "--hold", "0"]);
    assert!(
        text(&out.stdout).contains("swap spare->work"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
}

#[test]
fn a_dry_run_changes_nothing_and_prints_only_would_and_cycle_lines() {
    let f = Fixture::new();
    let before = snapshot(&f);
    let out = f.ok(&["profile", "rotate", "--dry-run"]);
    assert_eq!(snapshot(&f), before, "dry run wrote or moved something");
    assert!(!f.rotate_dir().exists());
    let ls = lines(&out);
    assert!(!ls.is_empty());
    for l in &ls {
        assert!(l.starts_with("would ") || l.starts_with("cycle "), "{l:?}");
        assert_log_grammar(l, false);
    }
    assert!(
        ls.iter().any(|l| l.starts_with("would swap work->spare")),
        "{ls:?}"
    );
}

#[test]
fn tuning_flags_before_a_subcommand_are_a_usage_error() {
    let f = Fixture::new();
    assert_eq!(
        f.run(&["profile", "rotate", "--margin", "3", "start"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        f.run(&["profile", "rotate", "start", "--dry-run"])
            .status
            .code(),
        Some(2)
    );
    assert!(!f.rotate_dir().join("rotate.pid").exists());
}

#[test]
fn a_bad_setting_names_the_flag_and_moves_nothing() {
    let f = Fixture::new();
    let out = f.run(&["profile", "rotate", "--session-at", "eighty"]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("invalid rotate setting --session-at \"eighty\""),
        "{err}"
    );
    assert!(
        err.contains("hint: run `amplihack profile rotate --help`"),
        "{err}"
    );

    let out = f
        .cmd(&["profile", "rotate"])
        .env("AMPLIHACK_ROTATE_WEEK_AT", "0")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("AMPLIHACK_ROTATE_WEEK_AT"));
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
}

#[test]
fn an_unknown_login_is_adopted_as_default_original() {
    let f = Fixture::new();
    let home_json = serde_json::json!({
        "oauthAccount": account_json("stranger"),
        "primaryApiKey": "SECRET-KEY-HOME"
    });
    private_file(&f.home.join(".claude.json"), &home_json.to_string());
    f.usage("stranger", 95, 10);
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        text(&out.stdout).contains("swap default-original->spare"),
        "{}",
        text(&out.stdout)
    );
    let adopted = f.root.join("default-original");
    assert_eq!(
        ino(&adopted.join(CREDS)),
        Some(f.live_ino),
        "moved, not copied"
    );
    let cj: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(adopted.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(
        cj.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["oauthAccount"]
    );
    assert_eq!(cj["oauthAccount"]["accountUuid"], "uuid-stranger");
    assert_eq!(ino(&f.slot("work")), None, "work was never live");
    f.assert_each_login_once();
}

#[test]
fn a_no_capacity_alert_is_written_once_across_runs() {
    let f = Fixture::new();
    f.usage("spare", 99, 99);
    f.usage("extra", 99, 99);
    for _ in 0..2 {
        let out = f.ok(&["profile", "rotate"]);
        assert!(!text(&out.stdout).contains("swap "));
    }
    let alerts = f
        .log()
        .lines()
        .filter(|l| l.contains("ALERT: no profile has capacity live=work s=90% w=40%"))
        .count();
    assert_eq!(alerts, 1, "{}", f.log());
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));

    // Once a cycle stays without alerting the key clears and it can recur.
    f.usage("work", 10, 10);
    f.ok(&["profile", "rotate"]);
    f.usage("work", 90, 40);
    f.ok(&["profile", "rotate"]);
    let alerts = f
        .log()
        .lines()
        .filter(|l| l.contains("ALERT: no profile"))
        .count();
    assert_eq!(alerts, 2);
}

#[test]
fn no_login_in_home_is_an_alert_and_nothing_moves_in() {
    let f = Fixture::new();
    fs::rename(f.home_login(), f.tmp.path().join("aside")).unwrap();
    let out = f.ok(&["profile", "rotate"]);
    assert!(text(&out.stdout).contains("ALERT: no login in ~/.claude; no swap"));
    assert!(!f.home_login().exists());
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
}

#[test]
fn a_hand_change_is_an_alert_and_nothing_moves() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate"]);
    // Someone ran /login in ~/.claude as `extra`'s account.
    let home_json = serde_json::json!({"oauthAccount": account_json("extra")});
    private_file(&f.home.join(".claude.json"), &home_json.to_string());
    f.usage("extra", 95, 95);
    let before = f.logins();
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        text(&out.stdout).contains(
            "ALERT: the account in ~/.claude is not the one the ledger names (live=spare); no swap"
        ),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(f.logins(), before);
}

#[test]
fn a_held_prototype_lock_refuses_rotate_but_not_a_dry_run() {
    let f = Fixture::new();
    let proto = f.root.join(".rotator");
    fs::create_dir(&proto).unwrap();
    fs::write(proto.join("lock"), "").unwrap();
    fs::write(proto.join("ledger.json"), r#"{"live":null}"#).unwrap();
    let holder = fs::File::open(proto.join("lock")).unwrap();
    // SAFETY: valid open descriptor.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let listing = |p: &Path| {
        let mut v: Vec<_> = fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        v.sort();
        v
    };
    let before = listing(&proto);

    let out = f.run(&["profile", "rotate"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("stop the external rotator (tmux session claude-rotator) first"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));

    let out = f.ok(&["profile", "rotate", "--dry-run"]);
    assert!(text(&out.stdout).contains("would "));
    assert_eq!(listing(&proto), before, "nothing created under .rotator");
    assert_eq!(fs::read_to_string(proto.join("lock")).unwrap(), "");
    assert_eq!(
        fs::read_to_string(proto.join("ledger.json")).unwrap(),
        r#"{"live":null}"#
    );
    drop(holder);
}

#[test]
fn profile_usage_attributes_home_to_the_rotated_in_profile() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate"]);
    let out = f.ok(&["profile", "usage", "--json"]);
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    let live: Vec<_> = rows
        .iter()
        .filter(|r| r["live_in"] == "~/.claude")
        .map(|r| r["profile"].as_str().unwrap())
        .collect();
    assert_eq!(live, ["spare"]);
    let spare = rows.iter().find(|r| r["profile"] == "spare").unwrap();
    assert_eq!(spare["session"]["used_percentage"], 10.0);
}

#[test]
fn probes_get_no_rotate_settings_and_the_live_probe_has_no_config_dir() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate", "--hold", "0"]);
    let envs = f.probe_envs();
    assert!(
        envs.iter().any(|e| e.contains("CLAUDE_CONFIG_DIR=<unset>")),
        "{envs:?}"
    );
    for e in &envs {
        assert!(!e.contains("AMPLIHACK_ROTATE"), "{e}");
    }
}
