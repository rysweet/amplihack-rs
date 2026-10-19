//! `<root>/.auto-switch`: parsing, rendering, env-over-file precedence and
//! the on-disk write rules (issue #1569).

use super::config::{self, Config, EnvToggle, FileState, Source};
use crate::commands::profile::name::ProfileName;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn names(list: &[&str]) -> Vec<ProfileName> {
    list.iter()
        .map(|n| ProfileName::parse(n).unwrap())
        .collect()
}

fn cfg(enabled: bool, threshold: u8, exclude: &[&str]) -> Config {
    Config {
        enabled,
        threshold,
        exclude: names(exclude),
    }
}

#[test]
fn default_is_off_at_ninety_with_no_exclusions() {
    assert_eq!(Config::default(), cfg(false, 90, &[]));
}

#[test]
fn parses_all_keys() {
    let c = config::parse(b"enabled=on\nthreshold=80\nexclude=a,b\n").unwrap();
    assert_eq!(c, cfg(true, 80, &["a", "b"]));
    let c = config::parse(b"enabled=off\n").unwrap();
    assert_eq!(c, cfg(false, 90, &[]), "missing keys take their defaults");
    let c = config::parse(b"enabled=on\nexclude=\n").unwrap();
    assert!(c.exclude.is_empty());
}

#[test]
fn threshold_bounds_are_inclusive() {
    assert_eq!(
        config::parse(b"enabled=on\nthreshold=1\n")
            .unwrap()
            .threshold,
        1
    );
    assert_eq!(
        config::parse(b"enabled=on\nthreshold=100\n")
            .unwrap()
            .threshold,
        100
    );
    for bad in ["0", "101", "-5", "9x", "", " 90"] {
        let raw = format!("enabled=on\nthreshold={bad}\n");
        assert!(config::parse(raw.as_bytes()).is_err(), "threshold={bad:?}");
    }
}

#[test]
fn corrupt_inputs_are_rejected() {
    for bad in [
        &b""[..],
        b"threshold=90\n",             // enabled is required
        b"enabled=yes\n",              // only on/off
        b"enabled=on\nenabled=on\n",   // repeated key
        b"enabled=on\nmode=fast\n",    // unknown key
        b"enabled=on\nexclude=../x\n", // invalid name
        b"enabled=on\nexclude=.hidden\n",
        b"enabled=on\njunk\n", // no '='
        b"enabled=\xffon\n",   // not UTF-8
    ] {
        assert!(
            config::parse(bad).is_err(),
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn oversized_input_is_corrupt() {
    let mut raw = b"enabled=on\nexclude=".to_vec();
    while raw.len() <= 4096 {
        raw.extend_from_slice(b"a,");
    }
    raw.extend_from_slice(b"a\n");
    assert!(config::parse(&raw).is_err());
}

#[test]
fn render_is_fixed_order_and_round_trips() {
    let c = cfg(true, 75, &["b", "a"]);
    let text = config::render(&c);
    assert_eq!(text, "enabled=on\nthreshold=75\nexclude=b,a\n");
    assert_eq!(config::parse(text.as_bytes()).unwrap(), c);
    assert_eq!(
        config::render(&Config::default()),
        "enabled=off\nthreshold=90\nexclude=\n"
    );
}

#[test]
fn env_toggle_values() {
    assert_eq!(config::env_toggle(None), EnvToggle::Unset);
    assert_eq!(config::env_toggle(Some(OsStr::new(""))), EnvToggle::Unset);
    assert_eq!(config::env_toggle(Some(OsStr::new("1"))), EnvToggle::On);
    assert_eq!(config::env_toggle(Some(OsStr::new("0"))), EnvToggle::Off);
    for other in ["true", "on", "yes", " 1", "2", "off"] {
        assert_eq!(
            config::env_toggle(Some(OsStr::new(other))),
            EnvToggle::Invalid,
            "{other:?}"
        );
    }
}

#[test]
fn precedence_table() {
    let on = FileState::Valid(cfg(true, 70, &["z"]));
    let off = FileState::Valid(cfg(false, 70, &[]));
    let cases = [
        (EnvToggle::On, &off, true, Source::Env),
        (EnvToggle::Off, &on, false, Source::Env),
        (EnvToggle::Unset, &on, true, Source::File),
        (EnvToggle::Unset, &off, false, Source::File),
        (EnvToggle::Invalid, &on, true, Source::File),
        (
            EnvToggle::Unset,
            &FileState::Missing,
            false,
            Source::Default,
        ),
        (
            EnvToggle::Invalid,
            &FileState::Missing,
            false,
            Source::Default,
        ),
        (
            EnvToggle::Unset,
            &FileState::Corrupt,
            false,
            Source::Corrupt,
        ),
        (EnvToggle::On, &FileState::Corrupt, true, Source::Env),
    ];
    for (env, file, want_on, want_source) in cases {
        let e = config::effective_from(env, file);
        assert_eq!(
            (e.on, e.source),
            (want_on, want_source),
            "{env:?} / {file:?}"
        );
    }
    // The threshold and exclusions always come from the file.
    let e = config::effective_from(EnvToggle::On, &off);
    assert_eq!(e.config.threshold, 70);
    let e = config::effective_from(EnvToggle::Off, &on);
    assert_eq!(e.config.exclude, names(&["z"]));
}

#[test]
fn source_labels_match_status_output() {
    assert_eq!(Source::Env.to_string(), "env");
    assert_eq!(Source::File.to_string(), "file");
    assert_eq!(Source::Default.to_string(), "default");
    assert_eq!(Source::Corrupt.to_string(), "corrupt");
}

#[test]
fn read_reports_missing_valid_and_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert_eq!(config::read(root), FileState::Missing);
    fs::write(root.join(".auto-switch"), "enabled=on\n").unwrap();
    assert_eq!(config::read(root), FileState::Valid(cfg(true, 90, &[])));
    fs::write(root.join(".auto-switch"), "enabled=sometimes\n").unwrap();
    assert_eq!(config::read(root), FileState::Corrupt);
    fs::remove_file(root.join(".auto-switch")).unwrap();
    fs::create_dir(root.join(".auto-switch")).unwrap();
    assert_eq!(config::read(root), FileState::Corrupt, "not a regular file");
}

#[test]
fn read_of_a_symlink_is_corrupt_and_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("real");
    fs::write(&target, "enabled=on\n").unwrap();
    std::os::unix::fs::symlink(&target, tmp.path().join(".auto-switch")).unwrap();
    assert_eq!(config::read(tmp.path()), FileState::Corrupt);
}

#[test]
fn write_is_private_atomic_and_byte_stable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let c = cfg(true, 85, &["a"]);
    config::write(root, &c).unwrap();
    let path = root.join(".auto-switch");
    let first = fs::read(&path).unwrap();
    assert_eq!(first, b"enabled=on\nthreshold=85\nexclude=a\n");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    config::write(root, &c).unwrap();
    assert_eq!(fs::read(&path).unwrap(), first);
    let leftovers: Vec<_> = fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .filter(|n| n != ".auto-switch")
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

#[test]
fn write_through_a_symlink_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("real");
    fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, tmp.path().join(".auto-switch")).unwrap();
    assert!(config::write(tmp.path(), &Config::default()).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    assert!(
        fs::symlink_metadata(tmp.path().join(".auto-switch"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn write_refuses_a_root_writable_by_others() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(config::write(&root, &Config::default()).is_err());
    assert!(!root.join(".auto-switch").exists());
}
