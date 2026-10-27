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
        overflow: Vec::new(),
    }
}

fn with_overflow(mut c: Config, overflow: &[&str]) -> Config {
    c.overflow = names(overflow);
    c
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

// --- overflow= (issue #1606) -----------------------------------------------

#[test]
fn overflow_parses_in_config_order() {
    let c = config::parse(b"enabled=on\nthreshold=80\nexclude=a\noverflow=zeta,alpha\n").unwrap();
    assert_eq!(c, with_overflow(cfg(true, 80, &["a"]), &["zeta", "alpha"]));
    assert_eq!(
        c.overflow,
        names(&["zeta", "alpha"]),
        "order is the ranking"
    );
}

#[test]
fn overflow_missing_or_empty_is_an_empty_list() {
    assert!(config::parse(b"enabled=on\n").unwrap().overflow.is_empty());
    assert!(
        config::parse(b"enabled=off\noverflow=\n")
            .unwrap()
            .overflow
            .is_empty()
    );
}

#[test]
fn overflow_corrupt_inputs_are_rejected() {
    for bad in [
        &b"enabled=on\noverflow=a\noverflow=b\n"[..], // repeated key
        b"enabled=on\noverflow=a,a\n",                // repeated name
        b"enabled=on\noverflow=../x\n",               // invalid name
        b"enabled=on\noverflow=.hidden\n",
        b"enabled=on\noverflow=a,\n", // empty name
        b"enabled=on\noverflow=,a\n",
    ] {
        assert!(
            config::parse(bad).is_err(),
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn render_without_overflow_is_byte_identical_to_before() {
    assert_eq!(
        config::render(&cfg(true, 75, &["b", "a"])),
        "enabled=on\nthreshold=75\nexclude=b,a\n"
    );
    assert_eq!(
        config::render(&Config::default()),
        "enabled=off\nthreshold=90\nexclude=\n"
    );
}

#[test]
fn render_writes_overflow_last_and_round_trips() {
    let c = with_overflow(cfg(false, 90, &["x"]), &["zeta", "alpha"]);
    let text = config::render(&c);
    assert_eq!(
        text,
        "enabled=off\nthreshold=90\nexclude=x\noverflow=zeta,alpha\n"
    );
    assert_eq!(config::parse(text.as_bytes()).unwrap(), c);
}

#[test]
fn rewriting_exclude_keeps_overflow() {
    // What the `auto-switch exclude/on/off` writers do: read, change one
    // field, write back.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        root.join(".auto-switch"),
        "enabled=on\nthreshold=90\nexclude=\noverflow=cred\n",
    )
    .unwrap();
    let FileState::Valid(mut c) = config::read(root) else {
        panic!("valid file expected");
    };
    c.exclude = names(&["old"]);
    c.enabled = false;
    config::write(root, &c).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".auto-switch")).unwrap(),
        "enabled=off\nthreshold=90\nexclude=old\noverflow=cred\n"
    );
}

#[test]
fn selection_exclude_is_exclude_then_overflow_without_duplicates() {
    let c = with_overflow(cfg(true, 90, &["b", "a"]), &["c", "a", "d"]);
    assert_eq!(c.selection_exclude(), names(&["b", "a", "c", "d"]));
    assert!(cfg(true, 90, &[]).selection_exclude().is_empty());
    assert_eq!(
        with_overflow(cfg(true, 90, &[]), &["c"]).selection_exclude(),
        names(&["c"])
    );
}

#[test]
fn effective_keeps_overflow_from_the_file_whatever_the_env_says() {
    let file = FileState::Valid(with_overflow(cfg(false, 90, &[]), &["cred"]));
    for env in [EnvToggle::On, EnvToggle::Off, EnvToggle::Unset] {
        assert_eq!(
            config::effective_from(env, &file).config.overflow,
            names(&["cred"]),
            "{env:?}"
        );
    }
    assert!(
        config::effective_from(EnvToggle::On, &FileState::Corrupt)
            .config
            .overflow
            .is_empty(),
        "a corrupt file gives no overflow (fail closed)"
    );
}

// --- Locked read-modify-write (issue #1606, D6) ----------------------------
//
// Contract:
// - `config::update(root, mode, f)` is the only writer of `.auto-switch`
//   outside tests. It takes the auto-switch state lock with
//   `State::try_lock(CONFIG_LOCK_WAIT)` (10 s), reads the file, applies `f`,
//   and writes only when the result differs from what was read. It returns
//   the resulting `Config`.
// - `config::update_within(root, mode, wait, f)` is the same with the wait
//   given, so the busy case can be tested without a 10 s sleep. When the
//   lock is not obtained it returns `AutoSwitchConfig` saying the file is
//   being changed by another process, and never writes unlocked.
// - `ReadMode::DefaultOnInvalid` (auto-switch on/off) starts from the
//   defaults when the file is missing or corrupt. `ReadMode::Editable`
//   (threshold, exclude, overflow add/remove) starts from the defaults when
//   it is missing and refuses a corrupt file, leaving it unchanged.
// - A render over `MAX_BYTES` (4096) is refused before writing.
// - `config::read` treats a file writable by group or others as `Corrupt`.

use super::config::{CONFIG_LOCK_WAIT, ReadMode};
use super::state::State;
use crate::commands::profile::ProfileError;
use std::time::Duration;

fn private_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    tmp
}

fn write_file(root: &std::path::Path, body: &str) {
    let p = root.join(".auto-switch");
    fs::write(&p, body).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
}

fn file_text(root: &std::path::Path) -> String {
    fs::read_to_string(root.join(".auto-switch")).unwrap()
}

#[test]
fn the_config_lock_waits_ten_seconds() {
    assert_eq!(CONFIG_LOCK_WAIT, Duration::from_secs(10));
}

#[test]
fn update_applies_the_change_and_returns_the_result() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=on\nthreshold=90\nexclude=x\n");
    let c = config::update(root, ReadMode::Editable, |c| {
        c.overflow.push(ProfileName::parse("cred").unwrap());
        Ok(())
    })
    .unwrap();
    assert_eq!(c.overflow, names(&["cred"]));
    assert_eq!(
        file_text(root),
        "enabled=on\nthreshold=90\nexclude=x\noverflow=cred\n"
    );
}

#[test]
fn update_of_a_missing_file_starts_from_the_defaults() {
    for mode in [ReadMode::Editable, ReadMode::DefaultOnInvalid] {
        let tmp = private_root();
        let root = tmp.path();
        config::update(root, mode, |c| {
            c.threshold = 70;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            file_text(root),
            "enabled=off\nthreshold=70\nexclude=\n",
            "{mode:?}"
        );
    }
}

#[test]
fn editable_update_refuses_a_corrupt_file_and_leaves_it() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=maybe\noverflow=cred\n");
    let mut called = false;
    let err = config::update(root, ReadMode::Editable, |_| {
        called = true;
        Ok(())
    })
    .unwrap_err();
    assert!(matches!(err, ProfileError::AutoSwitchConfig(_)), "{err:?}");
    assert!(!called, "the closure never sees a corrupt file");
    assert_eq!(file_text(root), "enabled=maybe\noverflow=cred\n");
}

#[test]
fn default_on_invalid_update_replaces_a_corrupt_file() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=maybe\n");
    config::update(root, ReadMode::DefaultOnInvalid, |c| {
        c.enabled = true;
        Ok(())
    })
    .unwrap();
    assert_eq!(file_text(root), "enabled=on\nthreshold=90\nexclude=\n");
}

#[test]
fn an_unchanged_config_is_not_written() {
    let tmp = private_root();
    let root = tmp.path();
    // Not the canonical render: a rewrite would change the bytes.
    write_file(root, "threshold=90\nenabled=on\n");
    let before = fs::symlink_metadata(root.join(".auto-switch")).unwrap();
    let c = config::update(root, ReadMode::Editable, |_| Ok(())).unwrap();
    assert!(c.enabled);
    let after = fs::symlink_metadata(root.join(".auto-switch")).unwrap();
    assert_eq!(file_text(root), "threshold=90\nenabled=on\n");
    assert_eq!(before.ino(), after.ino(), "no atomic replace happened");
}

#[test]
fn an_error_from_the_closure_writes_nothing() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=on\nthreshold=90\nexclude=\n");
    let err = config::update(root, ReadMode::Editable, |c| {
        c.threshold = 10;
        Err(ProfileError::AutoSwitchConfig("no".into()))
    })
    .unwrap_err();
    assert!(matches!(err, ProfileError::AutoSwitchConfig(_)));
    assert_eq!(file_text(root), "enabled=on\nthreshold=90\nexclude=\n");
}

#[test]
fn a_render_over_the_size_limit_is_refused_and_the_file_kept() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=on\nthreshold=90\nexclude=\n");
    let err = config::update(root, ReadMode::Editable, |c| {
        for i in 0..400 {
            c.exclude
                .push(ProfileName::parse(&format!("profile-{i}")).unwrap());
        }
        Ok(())
    })
    .unwrap_err();
    assert!(matches!(err, ProfileError::AutoSwitchConfig(_)), "{err:?}");
    assert_eq!(file_text(root), "enabled=on\nthreshold=90\nexclude=\n");
}

#[test]
fn update_reports_busy_when_the_lock_is_held_and_leaves_the_file() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=on\nthreshold=90\nexclude=\n");
    let state = State::new(root).unwrap();
    let _held = state.lock().unwrap();
    let mut called = false;
    let err = config::update_within(root, ReadMode::Editable, Duration::ZERO, |c| {
        called = true;
        c.threshold = 50;
        Ok(())
    })
    .unwrap_err();
    let ProfileError::AutoSwitchConfig(msg) = &err else {
        panic!("{err:?}");
    };
    assert!(msg.contains("another process"), "{msg}");
    assert!(msg.contains("retry"), "{msg}");
    assert!(!called, "nothing is read or changed without the lock");
    assert_eq!(file_text(root), "enabled=on\nthreshold=90\nexclude=\n");
}

#[test]
fn update_waits_for_a_lock_released_within_the_wait() {
    let tmp = private_root();
    let root = tmp.path().to_path_buf();
    write_file(&root, "enabled=on\nthreshold=90\nexclude=\n");
    let state = State::new(&root).unwrap();
    let held = state.lock().unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
    });
    config::update_within(&root, ReadMode::Editable, Duration::from_secs(5), |c| {
        c.threshold = 50;
        Ok(())
    })
    .unwrap();
    release.join().unwrap();
    assert_eq!(file_text(&root), "enabled=on\nthreshold=50\nexclude=\n");
}

#[test]
fn concurrent_updates_keep_both_changes() {
    let tmp = private_root();
    let root = tmp.path().to_path_buf();
    write_file(&root, "enabled=on\nthreshold=90\nexclude=\n");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let spawn = |field: &'static str| {
        let root = root.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            config::update(&root, ReadMode::Editable, |c| {
                // Widen the read-modify-write window.
                std::thread::sleep(Duration::from_millis(50));
                match field {
                    "overflow" => c.overflow.push(ProfileName::parse("cred").unwrap()),
                    _ => c.exclude.push(ProfileName::parse("old").unwrap()),
                }
                Ok(())
            })
            .unwrap();
        })
    };
    let a = spawn("overflow");
    let b = spawn("exclude");
    a.join().unwrap();
    b.join().unwrap();
    assert_eq!(
        file_text(&root),
        "enabled=on\nthreshold=90\nexclude=old\noverflow=cred\n"
    );
}

#[test]
fn a_file_writable_by_others_reads_as_corrupt() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=on\nthreshold=90\nexclude=\noverflow=cred\n");
    fs::set_permissions(root.join(".auto-switch"), fs::Permissions::from_mode(0o602)).unwrap();
    assert_eq!(config::read(root), FileState::Corrupt);
    assert!(
        config::effective_from(EnvToggle::Unset, &config::read(root))
            .config
            .overflow
            .is_empty(),
        "an unsafe file gives no overflow profiles"
    );
}

#[test]
fn a_private_file_still_reads_as_valid() {
    let tmp = private_root();
    let root = tmp.path();
    write_file(root, "enabled=on\nthreshold=90\nexclude=\noverflow=cred\n");
    let FileState::Valid(c) = config::read(root) else {
        panic!("valid file expected");
    };
    assert_eq!(c.overflow, names(&["cred"]));
}

/// Every non-test source under `profile/` other than `config.rs` reaches
/// `.auto-switch` through `config::update`, so no writer skips the lock.
#[test]
fn config_write_is_called_only_from_update() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile");
    let mut files = Vec::new();
    walk(&base, &mut files);
    let mut offenders = Vec::new();
    for p in &files {
        let name = p.file_name().unwrap().to_string_lossy();
        if name.starts_with("tests_") || name == "config.rs" {
            continue;
        }
        let text = fs::read_to_string(p).unwrap();
        if text.contains("config::write(") {
            offenders.push(p.strip_prefix(&base).unwrap().display().to_string());
        }
    }
    assert!(offenders.is_empty(), "unlocked writers: {offenders:?}");
}
