//! Explicit reset preserves raw metadata before startup probes and self-heal.
#![cfg(unix)]
use crate::issue_2284_codex_recovery::fixture::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn reset_archives_malformed_bytes_reports_destination_and_launches_without_forwarding_flag() {
    check_retained_reset(false);
    check_retained_reset(true);
}

fn check_retained_reset(valid_journal: bool) {
    let f = LaunchFixture::new();
    let bytes = if valid_journal {
        let bytes = f.orphan();
        // Both unfamiliar targets contradict every recorded state, with no
        // later completion proof. Automatic recovery must retain this journal.
        fs::write(f.codex.join("config.toml"), b"# foreign in-flight edit\n").unwrap();
        fs::write(
            f.codex.join("hooks.json"),
            b"{\"hooks\":{},\"foreign\":true}",
        )
        .unwrap();
        bytes
    } else {
        b"\xff\0 interrupted journal\n  \x1b[31m\n".to_vec()
    };
    f.pending(&bytes);
    fs::set_permissions(
        f.root.join("pending.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let out = f.launch(&["--reset-recovery"], &["exec", "-"]);
    let argv = f.assert_launched(&out);
    let archive = f.assert_report(&out, &bytes);
    assert_eq!(
        fs::metadata(archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(argv.iter().any(|arg| arg == "exec"));
    assert!(!argv.iter().any(|arg| arg == "--reset-recovery"));
    let order = fs::read_to_string(f.home.join("probe-order")).unwrap();
    assert!(
        order.lines().all(|line| line == "clear"),
        "reset must precede even client resolution: {order}"
    );
}

#[test]
fn reset_without_pending_reports_no_move_and_launches() {
    let f = LaunchFixture::new();
    let out = f.launch(&["--reset-recovery"], &["resume", "--last"]);
    let argv = f.assert_launched(&out);
    assert_eq!(argv, ["resume", "--last"]);
    assert!(f.archives().is_empty());
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        text.lines().filter(|line| *line == NO_PENDING).count(),
        1,
        "{text}"
    );
    assert!(!text.contains(PRESERVED), "{text}");
}

#[test]
fn reset_is_metadata_only_and_does_not_inspect_selected_codex_targets() {
    let f = LaunchFixture::new();
    f.pending(b"unparseable metadata with /outside/paths");
    let package = f.root.join("market/plugin");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("foreign"), b"foreign package").unwrap();
    fs::write(f.root.join("ownership.json"), b"foreign ownership bytes").unwrap();
    let profile = f.home.join(".codex-profiles/keep");
    fs::create_dir_all(profile.parent().unwrap()).unwrap();
    fs::write(&profile, b"profile").unwrap();
    let config = fs::read(f.codex.join("config.toml")).unwrap();
    let hooks = fs::read(f.codex.join("hooks.json")).unwrap();
    // A native-target symlink must not be traversed by metadata-only reset.
    fs::remove_file(f.codex.join("hooks.json")).unwrap();
    let outside = f.home.join("outside-hooks");
    fs::write(&outside, &hooks).unwrap();
    std::os::unix::fs::symlink(&outside, f.codex.join("hooks.json")).unwrap();
    let out = f.launch(&["--reset-recovery"], &["resume", "--last"]);
    f.assert_launched(&out);
    f.assert_report(&out, b"unparseable metadata with /outside/paths");
    assert_eq!(fs::read(f.codex.join("config.toml")).unwrap(), config);
    assert_eq!(fs::read(outside).unwrap(), hooks);
    assert!(
        fs::symlink_metadata(f.codex.join("hooks.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(package.join("foreign")).unwrap(),
        b"foreign package"
    );
    assert_eq!(
        fs::read(f.root.join("ownership.json")).unwrap(),
        b"foreign ownership bytes"
    );
    assert_eq!(fs::read(profile).unwrap(), b"profile");
}

#[test]
fn reset_runs_before_startup_self_heal_even_when_self_heal_fails() {
    let f = LaunchFixture::new();
    let bytes = b"malformed pending that previously blocked startup";
    f.pending(bytes);
    // Deterministic unrelated self-heal failure, before any fetch or install.
    fs::create_dir(f.home.join(".amplihack/.installed-version")).unwrap();
    let out = f
        .command()
        .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
        .args(["codex", "--reset-recovery", "--", "exec", "-"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    f.assert_report(&out, bytes);
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("self-heal failed") && error.contains(".installed-version"),
        "{}",
        diagnostic(&out)
    );
    assert!(!f.home.join("launch-argv").exists());
    let order = fs::read_to_string(f.home.join("probe-order")).unwrap();
    assert!(order.lines().all(|line| line == "clear"), "{order}");
}

#[test]
fn native_flag_after_separator_does_not_reset_metadata() {
    let f = LaunchFixture::new();
    let bytes = b"journal must stay";
    f.pending(bytes);
    let out = f.launch(&[], &["--reset-recovery", "native-literal"]);
    assert_eq!(
        f.assert_launched(&out),
        ["--reset-recovery", "native-literal"]
    );
    assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
    assert!(f.archives().is_empty());
}

#[test]
fn unsafe_pending_entries_fail_closed_before_launch_and_name_the_path() {
    for kind in ["directory", "symlink"] {
        let f = LaunchFixture::new();
        let path = f.root.join("pending.json");
        let outside = f.home.join("outside-journal");
        fs::write(&outside, b"outside evidence").unwrap();
        if kind == "directory" {
            fs::create_dir(&path).unwrap();
        } else {
            std::os::unix::fs::symlink(&outside, &path).unwrap();
        }
        let out = f.launch(&["--reset-recovery"], &["exec", "-"]);
        assert!(!out.status.success(), "{kind}: {}", diagnostic(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(path.to_str().unwrap()),
            "{}",
            diagnostic(&out)
        );
        assert!(fs::symlink_metadata(&path).is_ok());
        assert_eq!(fs::read(outside).unwrap(), b"outside evidence");
        assert!(f.archives().is_empty());
        assert!(!f.home.join("launch-argv").exists());
    }
}

#[test]
fn reset_refuses_unsafe_metadata_ancestor_without_following_it() {
    let f = LaunchFixture::new();
    let outside = f.home.join("outside-metadata");
    fs::rename(&f.root, &outside).unwrap();
    fs::write(outside.join("pending.json"), b"keep outside journal").unwrap();
    std::os::unix::fs::symlink(&outside, &f.root).unwrap();
    let out = f.launch(&["--reset-recovery"], &["exec", "-"]);
    assert!(!out.status.success(), "{}", diagnostic(&out));
    assert_eq!(
        fs::read(outside.join("pending.json")).unwrap(),
        b"keep outside journal"
    );
    assert!(!f.home.join("launch-argv").exists());
}

#[test]
fn reset_uses_home_metadata_despite_overridden_amplihack_home() {
    let f = LaunchFixture::new();
    let bytes = b"selected HOME journal";
    f.pending(bytes);
    let alternative = f.home.join("alternative-amplihack");
    fs::create_dir_all(alternative.join("codex")).unwrap();
    fs::write(alternative.join("codex/pending.json"), b"other metadata").unwrap();
    let out = f
        .command()
        .env("AMPLIHACK_HOME", &alternative)
        .args([
            "codex",
            "--subprocess-safe",
            "--no-reflection",
            "--reset-recovery",
            "--",
            "resume",
            "--last",
        ])
        .output()
        .unwrap();
    f.assert_launched(&out);
    f.assert_report(&out, bytes);
    assert_eq!(
        fs::read(alternative.join("codex/pending.json")).unwrap(),
        b"other metadata"
    );
}

#[test]
fn help_and_failed_parsing_do_not_reset_metadata() {
    for args in [
        vec!["codex", "--reset-recovery", "--help"],
        vec!["codex", "--max-turns", "0", "--reset-recovery"],
    ] {
        let f = LaunchFixture::new();
        let bytes = b"keep until successful parsing";
        f.pending(bytes);
        let out = f.command().args(&args).output().unwrap();
        assert_eq!(
            fs::read(f.root.join("pending.json")).unwrap(),
            bytes,
            "{args:?}: {}",
            diagnostic(&out)
        );
        assert!(f.archives().is_empty());
        assert!(!f.home.join("launch-argv").exists());
    }
}

#[test]
fn reset_refuses_symlink_caller_home() {
    let f = LaunchFixture::new();
    let bytes = b"preserve selected HOME evidence";
    f.pending(bytes);
    let alias = f.home.parent().unwrap().join("home-alias");
    std::os::unix::fs::symlink(&f.home, &alias).unwrap();
    let out = f
        .command()
        .env("HOME", &alias)
        .args(["codex", "--reset-recovery", "--", "exec", "-"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", diagnostic(&out));
    assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
    assert!(f.archives().is_empty());
    assert!(!f.home.join("launch-argv").exists());
}

#[test]
fn archive_status_sanitizes_control_characters_into_one_line() {
    let f = LaunchFixture::new_named("home-\n\x1b[31m");
    let bytes = b"raw evidence";
    f.pending(bytes);
    let out = f.launch(&["--reset-recovery"], &["resume", "--last"]);
    f.assert_launched(&out);
    let archives = f.archives();
    assert_eq!(archives.len(), 1, "{}", diagnostic(&out));
    assert_eq!(fs::read(&archives[0]).unwrap(), bytes);
    let sanitized: String = archives[0]
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let lines = text
        .lines()
        .filter(|line| line.starts_with(PRESERVED))
        .collect::<Vec<_>>();
    assert_eq!(lines, [format!("{PRESERVED}{sanitized}")]);
    assert!(!lines[0].chars().any(char::is_control));
}

#[test]
fn reset_retains_journal_when_package_lock_is_unsafe() {
    for kind in ["symlink", "hardlink"] {
        let f = LaunchFixture::new();
        let bytes = b"journal protected by package coordination";
        f.pending(bytes);
        let outside = f.home.join("outside-lock");
        fs::write(&outside, b"foreign lock bytes").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        let lock = f.home.join(".amplihack/codex.install.lock");
        if kind == "symlink" {
            std::os::unix::fs::symlink(&outside, &lock).unwrap();
        } else {
            fs::hard_link(&outside, &lock).unwrap();
        }
        let out = f.launch(&["--reset-recovery"], &["exec", "-"]);
        assert!(!out.status.success(), "{kind}: {}", diagnostic(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(lock.to_str().unwrap()),
            "{}",
            diagnostic(&out)
        );
        assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
        assert_eq!(fs::read(&outside).unwrap(), b"foreign lock bytes");
        assert_eq!(
            fs::metadata(outside).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(f.archives().is_empty());
        assert!(!f.home.join("launch-argv").exists());
    }
}
