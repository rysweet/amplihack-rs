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

/// Issue #1698: the rotator warns about a bad probe variable, the deadline as
/// well as the pool size, escaped, and the cycle still runs on the defaults.
#[test]
fn bad_probe_variables_warn_and_the_cycle_uses_the_defaults() {
    let f = Fixture::new();
    f.usage("work", 20, 20);
    let out = f
        .cmd(&["profile", "rotate"])
        .env("AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS", "soon\n")
        .env("AMPLIHACK_PROFILE_USAGE_PARALLEL", "lots")
        .output()
        .unwrap();
    let err = text(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains(
            "warning: ignored AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS \"soon\\n\" (use 1 to 600); using 120"
        ),
        "{err}"
    );
    assert!(
        err.contains("warning: ignored AMPLIHACK_PROFILE_USAGE_PARALLEL \"lots\""),
        "{err}"
    );
    assert!(text(&out.stdout).contains("cycle live=work"));
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
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
    // Issue #1659: when measuring began, for the limit-resume hook.
    let (measured, at) = (
        ledger["last_cycle"]["measured_at"].as_i64(),
        ledger["last_cycle"]["at"].as_i64(),
    );
    assert!(
        measured.is_some_and(|m| at.is_some_and(|at| 0 < m && m <= at)),
        "{ledger}"
    );
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

/// Issue #1693: a manual `/login` in `~/.claude` as `extra`'s account left
/// `spare` (the ledger's live profile) with no login and two copies of
/// `extra`'s account. One `profile rotate` moves `extra`'s own copy aside,
/// records `extra` as live, warns that `spare` has no login, and then, in the
/// same run, swaps to `work`, which has capacity again.
#[test]
fn a_hand_change_to_a_logged_in_profile_recovers_and_swaps_in_the_same_run() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate"]);
    assert_eq!(f.where_is(f.spare_ino).as_deref(), Some("~/.claude"));
    // Someone ran /login in ~/.claude as `extra`'s account.
    let home_json = serde_json::json!({"oauthAccount": account_json("extra")});
    private_file(&f.home.join(".claude.json"), &home_json.to_string());
    f.usage("extra", 95, 95);
    f.usage("work", 10, 10);
    let mut before: Vec<u64> = f.logins().iter().map(|(_, i)| *i).collect();
    before.sort();
    let out = f.ok(&["profile", "rotate"]);
    let stdout = text(&out.stdout);
    let at = |want: &str| {
        stdout
            .find(want)
            .unwrap_or_else(|| panic!("missing {want:?} in {stdout}"))
    };
    let recovered = at(
        "recovered: ~/.claude and extra held the same account; kept ~/.claude, moved extra's copy to ",
    );
    at(
        "ALERT: profile spare has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login spare",
    );
    let swapped = at("swap extra->work ");
    assert!(recovered < swapped, "recovery comes first: {stdout}");
    assert!(!stdout.contains("(live=spare); no swap"), "{stdout}");
    assert!(!stdout.contains("no-capacity"), "{stdout}");

    // `work` is live; the manual login went to `extra`'s slot.
    assert_eq!(f.where_is(f.live_ino).as_deref(), Some("~/.claude"));
    assert_eq!(f.where_is(f.spare_ino).as_deref(), Some("extra"));
    assert_eq!(f.home_account(), "uuid-work");
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["live"], "work", "{ledger}");
    assert_eq!(ledger["last_cycle"]["outcome"], "swapped", "{ledger}");
    // Every login still exists exactly once: in a profile, in ~/.claude or
    // in the backup under .rejected-logins, where `extra`'s copy now is.
    let mut backups = Vec::new();
    for e in fs::read_dir(f.root.join(".rejected-logins")).unwrap() {
        if let Ok(m) = fs::symlink_metadata(e.unwrap().path().join(CREDS)) {
            backups.push(m.ino());
        }
    }
    assert_eq!(backups, vec![f.extra_ino]);
    let mut after: Vec<u64> = f.logins().iter().map(|(_, i)| *i).collect();
    after.extend(backups);
    after.sort();
    assert_eq!(after, before, "no login lost or copied");
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
    // The fixture's autostart opt-out is itself an `AMPLIHACK_ROTATE*`
    // variable; the one-shot form never autostarts, so drop it here.
    let mut c = f.cmd(&["profile", "rotate", "--hold", "0"]);
    c.env_remove("AMPLIHACK_ROTATE_AUTOSTART");
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let envs = f.probe_envs();
    assert!(
        envs.iter().any(|e| e.contains("CLAUDE_CONFIG_DIR=<unset>")),
        "{envs:?}"
    );
    for e in &envs {
        assert!(!e.contains("AMPLIHACK_ROTATE"), "{e}");
    }
}

/// A stand-in for a rotator started from an older amplihack binary: a
/// process whose command line names `profile rotate watch` but whose
/// executable is not this build's. Killed on drop.
struct OldRotator(std::process::Child);

impl OldRotator {
    fn start(f: &Fixture) -> Self {
        // A loop keeps `sh` itself running (it never exec's into `sleep`,
        // which would replace the command line the rotator check reads), and
        // the short sleep means no orphan outlives the kill by more than a
        // second. No inherited pipes, so nextest sees no leak.
        let child = std::process::Command::new("sh")
            .args([
                "-c",
                "while :; do sleep 1; done",
                "amplihack",
                "profile",
                "rotate",
                "watch",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let dir = f.rotate_dir();
        fs::create_dir_all(&dir).unwrap();
        for d in [dir.parent().unwrap(), dir.as_path()] {
            fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        }
        fs::write(dir.join("rotate.pid"), format!("{}\n", child.id())).unwrap();
        Self(child)
    }
}

impl Drop for OldRotator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Issue #1659: beside a rotator from another binary, a one-shot cycle says
/// how to restart it and writes the ledger without fields that binary would
/// reject, so it never reads the ledger as inconsistent.
#[test]
fn a_one_shot_beside_an_older_rotator_keeps_the_ledger_readable_to_it() {
    let f = Fixture::new();
    f.usage("work", 20, 20);
    let _old = OldRotator::start(&f);
    let out = f.ok(&["profile", "rotate"]);
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("runs another amplihack binary")
            && stderr.contains("amplihack profile rotate stop && amplihack profile rotate start"),
        "{stderr}"
    );
    let ledger = f.ledger().expect("last_cycle recorded");
    assert_eq!(ledger["last_cycle"]["outcome"], "stayed");
    assert!(
        ledger["last_cycle"].get("measured_at").is_none(),
        "{ledger}"
    );
}
