//! Unit tests for `store.rs` (E1): the private directories (S20), `load`
//! with its two stages and content checks (C9, B9, C1), and `update` with
//! its write checks (C2). Every refused file is left byte for byte as it was.
//!
//! API under test:
//! - `AgentsDir::path(home: &Path) -> PathBuf` (`home/.amplihack/agents`)
//! - `AgentsDir::open(home: &Path) -> anyhow::Result<AgentsDir>` (creates as
//!   needed) and `AgentsDir::existing(home: &Path) ->
//!   anyhow::Result<Option<AgentsDir>>` (never creates)
//! - `AgentsDir::{registry, registry_lock, supervisor_lock, log, ended}(&self)
//!   -> PathBuf`
//! - `LoadError { Unsafe(String), Corrupt(String), TooNew(u64),
//!   Io(std::io::Error) }`, `Display` = the step 5b section 9.2 message
//! - `load(dir: &AgentsDir) -> Result<Registry, LoadError>`
//! - `update<T>(dir: &AgentsDir, now: i64, f: impl FnOnce(&mut Registry) ->
//!   anyhow::Result<T>) -> anyhow::Result<T>`

use super::registry::*;
use super::store::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";
/// 2026-10-30T09:12:04Z.
const T0: i64 = 1_793_351_524;
const KEPT: &str = "it was not changed";

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}
fn chmod(p: &Path, m: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
}

fn entry_json() -> Value {
    json!({
        "state": "active", "tmux_session": "coord", "tmux_window": "main",
        "launch_dir": "/srv/project", "conversation_id": ID, "skip_permissions": true,
        "registered_at": "2026-10-30T09:12:04Z", "launched_at": "2026-10-30T09:12:04Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d", "restarts": []
    })
}

/// A home with a private `agents/` holding `body` as `registry.json` (0600).
fn with_registry(body: &str) -> (tempfile::TempDir, AgentsDir) {
    let home = tempfile::tempdir().unwrap();
    let dir = AgentsDir::open(home.path()).unwrap();
    fs::write(dir.registry(), body).unwrap();
    chmod(&dir.registry(), 0o600);
    (home, dir)
}

/// The whole chain of an expected error.
fn err<T>(r: anyhow::Result<T>) -> String {
    format!("{:#}", r.err().expect("an error"))
}
/// The refusals of `open` and of `existing` for `home`, joined.
fn refusals(home: &Path) -> String {
    let (open, existing) = (AgentsDir::open(home), AgentsDir::existing(home));
    format!("{} / {}", err(open), err(existing))
}
/// `<registry path>: <rest>`, the shape of every registry message.
fn msg(dir: &AgentsDir, rest: &str) -> String {
    format!("{}: {rest}", dir.registry().display())
}

fn doc(agent: Value) -> String {
    json!({ "version": 1, "agents": { "coordinator": agent } }).to_string()
}

#[test]
fn open_creates_private_directories_and_names_the_files() {
    let home = tempfile::tempdir().unwrap();
    let dir = AgentsDir::open(home.path()).unwrap();
    let agents = home.path().join(".amplihack/agents");
    assert_eq!(AgentsDir::path(home.path()), agents);
    let modes = (mode(&agents), mode(agents.parent().unwrap()));
    assert_eq!(modes, (0o700, 0o700));
    assert_eq!(dir.registry(), agents.join("registry.json"));
    assert_eq!(dir.registry_lock(), agents.join("registry.lock"));
    assert_eq!(dir.supervisor_lock(), agents.join("supervisor.lock"));
    assert_eq!(dir.log(), agents.join("supervisor.log"));
    assert_eq!(dir.ended(), agents.join("ended"));
}

#[test]
fn open_and_existing_refuse_an_unsafe_amplihack_dir() {
    type Setup = (&'static str, fn(&Path));
    let setups: [Setup; 3] = [
        ("is a symlink", |top| {
            let real = top.with_file_name("real");
            fs::create_dir(&real).unwrap();
            std::os::unix::fs::symlink(&real, top).unwrap();
        }),
        ("is not a directory", |top| fs::write(top, "").unwrap()),
        ("writable by other users", |top| {
            fs::create_dir(top).unwrap();
            chmod(top, 0o777);
        }),
    ];
    for (text, setup) in setups {
        let home = tempfile::tempdir().unwrap();
        setup(&home.path().join(".amplihack"));
        let both = refusals(home.path());
        assert_eq!(both.matches(text).count(), 2, "{both}");
    }
}

#[test]
fn an_agents_dir_that_is_not_private_is_refused_not_repaired() {
    let home = tempfile::tempdir().unwrap();
    AgentsDir::open(home.path()).unwrap();
    let agents = AgentsDir::path(home.path());
    chmod(&agents, 0o750);
    let both = refusals(home.path());
    let text = "is not private (owner-only mode 0700)";
    assert_eq!(both.matches(text).count(), 2, "{both}");
    assert_eq!(mode(&agents), 0o750);
}

#[test]
fn existing_never_creates_and_open_keeps_an_existing_mode() {
    let home = tempfile::tempdir().unwrap();
    assert!(AgentsDir::existing(home.path()).unwrap().is_none());
    let top = home.path().join(".amplihack");
    assert!(!top.exists());
    fs::create_dir(&top).unwrap();
    chmod(&top, 0o755);
    let reg = load(&AgentsDir::open(home.path()).unwrap()).unwrap();
    assert_eq!((reg.version, reg.agents.len(), mode(&top)), (1, 0, 0o755));
    assert!(AgentsDir::existing(home.path()).unwrap().is_some());
}

#[test]
fn unsafe_files_are_refused() {
    let (_h, dir) = with_registry(&doc(entry_json()));
    chmod(&dir.registry(), 0o640);
    let e = load(&dir).unwrap_err();
    assert!(matches!(e, LoadError::Unsafe(_)), "{e}");
    let want = msg(
        &dir,
        "must be a regular file owned by you with mode 0600; it was not changed",
    );
    assert_eq!(e.to_string(), want);

    let (h, dir) = with_registry("");
    let real = h.path().join("elsewhere.json");
    fs::write(&real, doc(entry_json())).unwrap();
    chmod(&real, 0o600);
    fs::remove_file(dir.registry()).unwrap();
    std::os::unix::fs::symlink(&real, dir.registry()).unwrap();
    assert!(matches!(load(&dir), Err(LoadError::Unsafe(_))), "a symlink");

    let (_h, dir) = with_registry(&" ".repeat(8 * 1024 * 1024 + 1));
    let over = load(&dir);
    assert!(matches!(over, Err(LoadError::Unsafe(_))), "over 8 MiB");
}

#[test]
fn invalid_json_gives_line_and_column_only() {
    let (_h, dir) = with_registry("{\"version\": 1,\n \"agents\": {\"x\": SECRETVALUE}}");
    let e = load(&dir).unwrap_err();
    assert!(matches!(e, LoadError::Corrupt(_)), "{e}");
    let text = e.to_string();
    let start = msg(&dir, "not valid JSON (line 2, column ");
    assert!(text.starts_with(&start), "{text}");
    assert!(text.ends_with("); it was not changed"), "{text}");
    assert!(!text.contains("SECRETVALUE"), "{text}");
}

#[test]
fn the_version_is_checked_before_the_structure() {
    let mut future = entry_json();
    future["state"] = json!("paused");
    let body = json!({ "version": 2, "agents": { "coordinator": future } }).to_string();
    let (_h, dir) = with_registry(&body);
    let e = load(&dir).unwrap_err();
    assert!(matches!(e, LoadError::TooNew(2)), "C9: {e}");
    let want =
        "written by a newer amplihack (registry version 2); upgrade amplihack. It was not changed";
    assert!(e.to_string().ends_with(want), "{e}");

    let (_h, dir) = with_registry(&doc(future));
    let e = load(&dir).unwrap_err().to_string();
    assert!(e.contains("line ") && e.contains("column "), "{e}");
    assert!(!e.contains("paused"), "serde quoted the value: {e}");

    for body in [r#"{"agents": {}}"#, r#"{"version": 0, "agents": {}}"#] {
        let (_h, dir) = with_registry(body);
        assert!(matches!(load(&dir), Err(LoadError::Corrupt(_))), "{body}");
    }
}

#[test]
fn field_rules_are_checked_on_load_without_quoting_values() {
    let mut bad_ts = entry_json();
    bad_ts["registered_at"] = json!("yesterday-ish");
    let mut bad_profile = entry_json();
    bad_profile["amplihack_flags"] = json!({ "profile": "../x" });
    for (agent, field, value) in [
        (bad_ts, "registered_at", "yesterday-ish"),
        (bad_profile, "amplihack_flags.profile", "../x"),
    ] {
        let (_h, dir) = with_registry(&doc(agent));
        let e = load(&dir).unwrap_err();
        assert!(matches!(e, LoadError::Corrupt(_)), "{e}");
        let want = msg(
            &dir,
            &format!("agent coordinator: field {field} is not valid; {KEPT}"),
        );
        assert_eq!(e.to_string(), want);
        assert!(!e.to_string().contains(value));
    }
    let body = json!({ "version": 1, "agents": { "bad name!": entry_json() } }).to_string();
    let (_h, dir) = with_registry(&body);
    let e = load(&dir).unwrap_err();
    let quoted = e.to_string().contains("bad name!");
    assert!(matches!(e, LoadError::Corrupt(_)) && !quoted, "{e}");
}

#[test]
fn update_writes_private_pretty_json_and_prunes_restarts() {
    let home = tempfile::tempdir().unwrap();
    let dir = AgentsDir::open(home.path()).unwrap();
    let mut e = entry_json();
    e["restarts"] = json!([fmt_ts(T0 - 90_000), fmt_ts(T0 - 60), fmt_ts(T0 + 3_600)]);
    let entry: Entry = serde_json::from_value(e).unwrap();
    let out = update(&dir, T0, |reg| {
        reg.agents.insert("coordinator".into(), entry);
        Ok(42)
    })
    .unwrap();
    assert_eq!(out, 42);
    let modes = (mode(&dir.registry()), mode(&dir.registry_lock()));
    let raw = fs::read_to_string(dir.registry()).unwrap();
    assert_eq!(modes, (0o600, 0o600));
    assert!(
        raw.ends_with("}\n") && raw.contains("\n  \"agents\""),
        "{raw}"
    );
    let reg = load(&dir).unwrap();
    assert_eq!(reg.agents["coordinator"].restarts, [fmt_ts(T0 - 60)]);
}

#[test]
fn a_failed_or_invalid_update_writes_nothing() {
    let (_h, dir) = with_registry(&doc(entry_json()));
    let before = fs::read(dir.registry()).unwrap();
    let e = err(update(&dir, T0, |reg| {
        reg.agents.get_mut("coordinator").unwrap().model = Some("-p".into());
        Ok(())
    }));
    let want = msg(
        &dir,
        "refusing to write agent coordinator: field model is not valid; it was not changed",
    );
    assert!(e.contains(&want), "C2: {e}");
    let failing: anyhow::Result<()> = update(&dir, T0, |reg| {
        reg.agents.clear();
        anyhow::bail!("closure failed")
    });
    assert!(failing.is_err());
    assert_eq!(fs::read(dir.registry()).unwrap(), before);
}

#[test]
fn update_never_overwrites_a_registry_it_cannot_load_or_lock() {
    for body in ["not json", r#"{"version": 9, "agents": {}}"#] {
        let (_h, dir) = with_registry(body);
        let r = update(&dir, T0, |reg| Ok(std::mem::replace(reg, Registry::new())));
        assert!(r.is_err(), "{body}");
        assert_eq!(fs::read_to_string(dir.registry()).unwrap(), body);
    }
    let (h, dir) = with_registry(&doc(entry_json()));
    let target = h.path().join("precious");
    fs::write(&target, "keep").unwrap();
    chmod(&target, 0o600);
    std::os::unix::fs::symlink(&target, dir.registry_lock()).unwrap();
    let e = err(update(&dir, T0, |_| Ok(())));
    assert!(e.contains("is a symlink"), "{e}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
}

#[test]
fn update_refuses_a_registry_over_8_mib() {
    let (_h, dir) = with_registry(&doc(entry_json()));
    let before = fs::read(dir.registry()).unwrap();
    let dirs: Vec<String> = (0..9_000)
        .map(|i| format!("/{}{i}", "d".repeat(1_000)))
        .collect();
    let e = err(update(&dir, T0, |reg| {
        reg.agents.get_mut("coordinator").unwrap().add_dirs = dirs;
        Ok(())
    }));
    let want = msg(&dir, "the registry would exceed 8 MiB; it was not changed");
    assert!(e.contains(&want), "{e}");
    assert_eq!(fs::read(dir.registry()).unwrap(), before);
}
