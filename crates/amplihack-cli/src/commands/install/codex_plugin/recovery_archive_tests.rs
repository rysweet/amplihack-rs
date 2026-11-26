//! Exact preservation and deterministic collision behavior at startup.
use super::recovery_test_support::*;
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn archive_preserves_exact_bytes_inode_and_restrictive_permissions_with_utc_name() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let pending = f.root.join("pending.json");
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::metadata(&pending).unwrap();
    with_archive_stamp(|| assert!(f.install().unwrap()));
    let archive = f.root.join(format!("pending.json.stale-{STAMP}"));
    assert_eq!(f.archives(), vec![archive.clone()]);
    assert_eq!(fs::read(&archive).unwrap(), bytes);
    let after = fs::metadata(&archive).unwrap();
    assert_eq!(
        (after.dev(), after.ino()),
        (before.dev(), before.ino()),
        "archive must rename the captured journal"
    );
    assert_eq!(after.permissions().mode() & 0o777, 0o600);
    chrono::NaiveDateTime::parse_from_str(STAMP, "%Y%m%dT%H%M%SZ").unwrap();
    f.assert_progress();
}

#[test]
fn collision_suffix_preserves_regular_file_directory_and_dangling_symlink() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let base = format!("pending.json.stale-{STAMP}");
    fs::write(f.root.join(&base), b"earlier journal").unwrap();
    fs::create_dir(f.root.join(format!("{base}.1"))).unwrap();
    let link = f.root.join(format!("{base}.2"));
    std::os::unix::fs::symlink("missing-earlier-archive", &link).unwrap();
    with_archive_stamp(|| assert!(f.install().unwrap()));
    assert_eq!(fs::read(f.root.join(&base)).unwrap(), b"earlier journal");
    assert!(f.root.join(format!("{base}.1")).is_dir());
    assert_eq!(
        fs::read_link(link).unwrap(),
        Path::new("missing-earlier-archive")
    );
    assert_eq!(fs::read(f.root.join(format!("{base}.3"))).unwrap(), bytes);
    assert_eq!(f.archives().len(), 4);
    f.assert_progress();
}

#[test]
fn archive_does_not_grant_ownership_of_foreign_packages() {
    let f = Fixture::new();
    f.orphan();
    let package = f.root.join("market/plugin/skills/marker");
    fs::write(&package, b"foreign package").unwrap();
    let error = format!("{:#}", f.install().unwrap_err());
    assert!(
        error.contains("package") && (error.contains("foreign") || error.contains("outside")),
        "{error}"
    );
    assert_eq!(fs::read(package).unwrap(), b"foreign package");
    // Either the startup safety gate or preparation can refuse; neither may
    // adopt or replace this package merely because both Codex targets vanished.
}

#[test]
fn disappearing_marketplace_or_ledger_alone_never_proves_orphaning() {
    for missing in ["ownership.json", "market/.agents/plugins/marketplace.json"] {
        let f = Fixture::new();
        assert!(f.install().unwrap());
        f.upgrade(b"abandoned");
        let bytes = f.plan();
        fs::remove_file(f.root.join(missing)).unwrap();
        f.assert_retained(&bytes);
    }
}

#[test]
fn strict_recovery_never_archives_even_when_targets_disappeared() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let error = recover_install(&f.root, &f.binary, &f.codex).unwrap_err();
    assert!(format!("{error:#}").contains("retained"));
    assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
    assert!(f.archives().is_empty());
}

#[test]
fn reset_option_is_consumed_before_native_auto_and_container_argument_building() {
    let _f = Fixture::new();
    for mode in [None, Some("--auto"), Some("--docker")] {
        let mut argv = vec!["amplihack", "codex", "--max-turns", "1"];
        if let Some(mode) = mode {
            argv.push(mode);
        }
        argv.extend(["--reset-recovery", "--", "exec", "-"]);
        let cli = crate::Cli::try_parse_from(argv).unwrap();
        match cli.command {
            crate::Commands::Codex { args, .. } => assert_eq!(
                args,
                ["exec", "-"],
                "mode={mode:?}: reset must never reach client arguments"
            ),
            other => panic!("unexpected command {other:?}"),
        }
    }
}

#[test]
fn explicit_separator_preserves_native_reset_spelling() {
    let _f = Fixture::new();
    let cli = crate::Cli::try_parse_from([
        "amplihack",
        "codex",
        "--max-turns",
        "1",
        "--",
        "--reset-recovery",
        "native-literal",
    ])
    .unwrap();
    match cli.command {
        crate::Commands::Codex { args, .. } => {
            assert_eq!(args, ["--reset-recovery", "native-literal"])
        }
        other => panic!("unexpected command {other:?}"),
    }
}

#[test]
fn disappearance_of_recorded_empty_file_is_distinct_from_legitimate_absence() {
    let f = Fixture::new();
    fs::write(f.codex.join("config.toml"), b"").unwrap();
    let bytes = f.plan();
    assert_eq!(
        f.journal()["config"],
        json!([]),
        "empty bytes are a present file"
    );
    assert!(f.journal()["snapshots"]["hooks"].is_null());
    fs::remove_file(f.codex.join("config.toml")).unwrap();
    assert!(
        f.install()
            .expect("an empty recorded file still contradicts disappearance")
    );
    assert_eq!(f.archives().len(), 1);
    assert_eq!(fs::read(&f.archives()[0]).unwrap(), bytes);
    f.assert_progress();
}
