//! Issue #1760, R9.2(6): `agent register` and `agent unregister` round trip
//! the registry (R2.2, R2.3, R2.6, C4), and the file has the documented
//! shape: version 1, flat fields, absent optionals left out, mode 0600 in
//! 0700 directories (R2.1).

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::{
    CONV_A, CONV_B, Fx, LAUNCH_ID, expect_code, now, parse_ts, stderr, stdout, ts,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
fn register_prints_one_line_and_writes_a_flat_version_1_entry() {
    let fx = Fx::new();
    let d = fx.dir("mistt");
    let before = now();
    let o = fx.register("coordinator", "coord", &d, CONV_A, &[]);
    expect_code(&o, 0);
    assert_eq!(
        stdout(&o),
        format!(
            "registered coordinator: session coord, window main, conversation {CONV_A}, directory {}\n",
            d.display()
        )
    );
    let reg = fx.registry();
    assert_eq!(reg["version"], json!(1));
    let e = &reg["agents"]["coordinator"];
    assert_eq!(e["state"], "active");
    assert_eq!(e["tmux_session"], "coord");
    assert_eq!(e["tmux_window"], "main");
    assert_eq!(e["launch_dir"], json!(d));
    assert_eq!(e["conversation_id"], CONV_A);
    assert_eq!(e["add_dirs"], json!([]));
    assert_eq!(e["skip_permissions"], json!(false));
    assert_eq!(e["env"], json!({}));
    assert_eq!(e["restarts"], json!([]));
    assert_eq!(e["amplihack_flags"]["subprocess_safe"], json!(false));
    assert_eq!(e["amplihack_flags"]["no_reflection"], json!(false));
    let id = e["launch_id"].as_str().unwrap();
    assert!(
        id.len() == 32
            && id
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "launch_id must be 32 lowercase hex characters: {id}"
    );
    let reg_at = parse_ts(e["registered_at"].as_str().unwrap());
    assert!(reg_at >= before - 1 && reg_at <= now() + 1, "{e}");
    assert_eq!(e["launched_at"], e["registered_at"]);
    // No `$TMUX` and no flags: nothing else is known, so nothing else is written.
    for absent in [
        "tmux_socket",
        "claude_pid",
        "proc_start",
        "model",
        "effort",
        "permission_mode",
        "lost_at",
        "ended_at",
        "ended_by",
        "last_recovery",
        "alert",
    ] {
        assert!(e.get(absent).is_none(), "{absent} should be left out: {e}");
    }
    assert!(
        fx.registry_raw().ends_with(b"\n"),
        "pretty JSON and a newline"
    );
}

#[test]
fn registry_files_are_private() {
    let fx = Fx::new();
    let o = fx.register("coordinator", "coord", &fx.dir("d"), CONV_A, &[]);
    expect_code(&o, 0);
    assert_eq!(mode(&fx.home.join(".amplihack")), 0o700);
    assert_eq!(mode(&fx.agents_dir()), 0o700);
    assert_eq!(mode(&fx.registry_path()), 0o600);
    assert_eq!(mode(&fx.agents_dir().join("registry.lock")), 0o600);
}

#[test]
fn flags_are_recorded_in_their_fields() {
    let fx = Fx::new();
    let d = fx.dir("d");
    fx.profile_dir("work");
    let o = fx.register(
        "area-docs",
        "docs",
        &d,
        CONV_B,
        &[
            "--model",
            "opus",
            "--effort",
            "high",
            "--add-dir",
            "notes",
            "--add-dir",
            "/srv/shared",
            "--permission-mode",
            "acceptEdits",
            "--profile",
            "work",
            "--subprocess-safe",
            "--no-reflection",
            "--env",
            "CLAUDE_CODE_EFFORT_LEVEL=high",
        ],
    );
    expect_code(&o, 0);
    let e = fx.agent("area-docs");
    assert_eq!(e["model"], "opus");
    assert_eq!(e["effort"], "high");
    assert_eq!(
        e["add_dirs"],
        json!([d.join("notes"), "/srv/shared"]),
        "a relative --add-dir is made absolute against the launch directory"
    );
    assert_eq!(e["permission_mode"], "acceptEdits");
    assert_eq!(e["skip_permissions"], json!(false));
    assert_eq!(
        e["amplihack_flags"],
        json!({ "subprocess_safe": true, "no_reflection": true, "profile": "work" })
    );
    assert_eq!(e["env"], json!({ "CLAUDE_CODE_EFFORT_LEVEL": "high" }));
}

#[test]
fn reregistering_replaces_the_entry_and_clears_its_history() {
    let fx = Fx::new();
    let d = fx.dir("d");
    let old = ts(now() - 600);
    let mut e = fx.entry("coord", &d, Some(CONV_A));
    e["state"] = json!("stopped");
    e["alert"] = json!("stopped: restart limit (3 restarts in the last hour)");
    e["restarts"] = json!([old]);
    e["lost_at"] = json!(old);
    e["claude_pid"] = json!(4242);
    e["proc_start"] = json!(777);
    e["last_recovery"] = json!({ "logged_at": old, "result": "stopped: restart limit" });
    e["unknown_entry_field"] = json!("dropped with the entry");
    let previous = parse_ts(e["registered_at"].as_str().unwrap());
    let mut other = fx.entry("other", &fx.dir("o"), Some(CONV_B));
    other["future_field"] = json!({ "kept": true });
    fx.write_registry(&json!({
        "version": 1,
        "future_top": [1, 2],
        "agents": { "coordinator": e, "other": other }
    }));

    let o = fx.register("coordinator", "coord", &d, CONV_A, &[]);
    expect_code(&o, 0);
    let reg = fx.registry();
    let e = &reg["agents"]["coordinator"];
    assert_eq!(e["state"], "active");
    assert_eq!(e["restarts"], json!([]));
    for cleared in [
        "alert",
        "lost_at",
        "ended_at",
        "ended_by",
        "last_recovery",
        "claude_pid",
        "proc_start",
        "unknown_entry_field",
    ] {
        assert!(e.get(cleared).is_none(), "{cleared} should be cleared: {e}");
    }
    assert_ne!(e["launch_id"], LAUNCH_ID, "a new launch_id");
    assert!(parse_ts(e["registered_at"].as_str().unwrap()) > previous);
    assert_eq!(e["launched_at"], e["registered_at"]);
    assert_eq!(
        reg["future_top"],
        json!([1, 2]),
        "unknown top-level keys kept"
    );
    assert_eq!(
        reg["agents"]["other"]["future_field"],
        json!({ "kept": true }),
        "unknown keys of other entries kept"
    );
}

#[test]
fn registering_twice_within_a_second_gives_a_later_registered_at() {
    let fx = Fx::new();
    let d = fx.dir("d");
    expect_code(&fx.register("a", "s", &d, CONV_A, &[]), 0);
    let first = fx.agent("a");
    expect_code(&fx.register("a", "s", &d, CONV_A, &[]), 0);
    let second = fx.agent("a");
    let (t1, t2) = (
        parse_ts(first["registered_at"].as_str().unwrap()),
        parse_ts(second["registered_at"].as_str().unwrap()),
    );
    assert!(t2 > t1, "C4: {t1} then {t2}");
    assert_eq!(second["launched_at"], second["registered_at"]);
    assert_ne!(first["launch_id"], second["launch_id"]);
}

#[test]
fn unregister_removes_only_that_entry() {
    let fx = Fx::new();
    expect_code(&fx.register("a", "sa", &fx.dir("a"), CONV_A, &[]), 0);
    expect_code(&fx.register("b", "sb", &fx.dir("b"), CONV_B, &[]), 0);
    let o = fx.run(&["agent", "unregister", "a"]);
    expect_code(&o, 0);
    assert_eq!(stdout(&o), "unregistered a\n");
    let agents = fx.registry()["agents"].clone();
    assert!(agents.get("a").is_none(), "{agents}");
    assert_eq!(agents["b"]["conversation_id"], CONV_B);
    assert!(fx.tmux_records().is_empty(), "unregister never runs tmux");
}

#[test]
fn unregister_of_an_unknown_name_exits_1() {
    let fx = Fx::new();
    expect_code(&fx.register("a", "sa", &fx.dir("a"), CONV_A, &[]), 0);
    let before = fx.registry_raw();
    let o = fx.run(&["agent", "unregister", "ghost"]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains("no registered agent named ghost"),
        "{}",
        stderr(&o)
    );
    assert!(stdout(&o).is_empty());
    assert_eq!(fx.registry_raw(), before, "nothing written");
}

#[test]
fn an_invalid_name_is_refused_without_repeating_it() {
    let fx = Fx::new();
    let d = fx.dir("d");
    let long = "n".repeat(65);
    for bad in ["a/b", "_hidden", long.as_str()] {
        let o = fx.register(bad, "s", &d, CONV_A, &[]);
        expect_code(&o, 1);
        let err = stderr(&o);
        assert!(err.contains("NAME: "), "{err}");
        assert!(!err.contains(bad), "the rejected NAME was repeated: {err}");
        let o = fx.run(&["agent", "unregister", bad]);
        expect_code(&o, 1);
        assert!(stderr(&o).contains("NAME: "), "{}", stderr(&o));
    }
    assert!(!fx.registry_path().exists());
}
