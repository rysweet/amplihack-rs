//! Explicit synthetic credential identity for legacy rotation fixtures.
//!
//! The reader accepts only fixture roots registered through `body()`, not
//! every path under `std::env::temp_dir()` (#1946).
// Shared fixtures also serve Linux-only tests; keep the portable tests enabled.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use super::super::identify::Account;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Fixture roots seeded through `body()`. Process-wide because cycle and
/// supervise tests read fixtures from spawned threads. Entries are never
/// removed; tempfile names are random, so a stale root is not reused.
static OWN: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn own() -> MutexGuard<'static, BTreeSet<PathBuf>> {
    OWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lexical, component-wise check made before any filesystem access.
fn owned(dir: &Path) -> bool {
    dir.is_absolute()
        && !dir.components().any(|c| c == Component::ParentDir)
        && own().iter().any(|root| dir.starts_with(root))
}

/// The fixture root of `path`, `temp_dir()/<first component>`. Panics
/// unless `path` has no `..` and lies at least `depth` components below
/// `temp_dir()`, so neither `temp_dir()` nor an ancestor is ever a root.
fn fixture_root(path: &Path, depth: usize) -> PathBuf {
    let tmp = std::env::temp_dir();
    let below = path
        .strip_prefix(&tmp)
        .expect("fixture path must live under temp_dir()");
    assert!(
        !path.components().any(|c| c == Component::ParentDir),
        "fixture path must not contain `..`"
    );
    assert!(
        below.components().count() >= depth,
        "fixture path must lie at least {depth} components below temp_dir()"
    );
    match below.components().next() {
        Some(Component::Normal(first)) => tmp.join(first),
        _ => panic!("fixture path must start with a plain directory below temp_dir()"),
    }
}

/// Register `root`, a tempdir directly below `temp_dir()`, for a fixture
/// that writes its synthetic identity without `body()` (the handfix
/// fixtures). The same validation as `body()` applies.
pub(in crate::commands::profile::rotate) fn own_root(root: &Path) {
    let granted = fixture_root(root, 1);
    assert_eq!(
        granted, root,
        "fixture root must sit directly below temp_dir()"
    );
    own().insert(granted);
}

pub(in crate::commands::profile::rotate) fn read(dir: &Path) -> Result<Account, &'static str> {
    if !owned(dir) {
        return Err("not a task fixture");
    }
    let raw = std::fs::read(dir.join(".credentials.json")).map_err(|_| "fake login missing")?;
    let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| "fake login invalid")?;
    Account::from_block(
        v.get("task1753FakeIdentity")
            .ok_or("fake identity missing")?,
    )
    .ok_or("fake identity invalid")
}

/// Seed fake moving identity from the fixture's initial, synthetic account.
/// Later session rewrites never affect the login's evidence.
pub(in crate::commands::profile::rotate) fn body(path: &Path) -> String {
    let root = fixture_root(path, 2);
    own().insert(root);
    let dir = path.parent().unwrap();
    let session = if dir.ends_with(".claude") {
        dir.parent().unwrap()
    } else {
        dir
    };
    let name = if dir.ends_with(".claude") {
        "work"
    } else {
        dir.file_name().unwrap().to_str().unwrap()
    };
    let account = Account::of_dir(session).map(|a| a.block()).unwrap_or_else(||
        serde_json::json!({"accountUuid": format!("u-{name}"), "emailAddress": format!("{name}@example.com")}));
    serde_json::json!({"claudeAiOauth": {"accessToken": "SENTINEL"}, "task1753FakeIdentity": account}).to_string()
}

/// Issue #1946: `read` admits only the fixture roots that `body()` seeded,
/// not every path under `std::env::temp_dir()`. Each negative test writes a
/// valid synthetic credential with `std::fs::write`, never with `body()`,
/// so the path check is the only reason it can be refused.
mod issue_1946_reader_scope {
    use super::{Account, body, read};
    use std::path::Path;

    const REFUSED: Result<(), &str> = Err("not a task fixture");

    fn valid(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"SENTINEL"},"task1753FakeIdentity":{"accountUuid":"u-x","emailAddress":"x@example.com"}}"#,
        )
        .unwrap();
    }

    /// Register `root` as a fixture the way every caller does: seed
    /// `root/home/.claude/.credentials.json` through `body()`.
    fn seed(root: &Path) {
        let claude = root.join("home/.claude");
        std::fs::create_dir_all(&claude).unwrap();
        let path = claude.join(".credentials.json");
        std::fs::write(&path, body(&path)).unwrap();
    }

    fn outcome(dir: &Path) -> Result<(), &'static str> {
        read(dir).map(|_| ())
    }

    fn account(name: &str) -> Account {
        Account::from_block(&serde_json::json!({
            "accountUuid": format!("u-{name}"),
            "emailAddress": format!("{name}@example.com"),
        }))
        .unwrap()
    }

    #[test]
    fn unregistered_tempdir_is_refused() {
        let b = tempfile::tempdir().unwrap();
        valid(b.path());
        assert_eq!(outcome(b.path()), REFUSED);
    }

    #[test]
    fn parent_dir_escape_is_refused() {
        let a = tempfile::tempdir().unwrap();
        seed(a.path());
        let b = tempfile::tempdir().unwrap();
        valid(b.path());
        let escape = a
            .path()
            .join("home")
            .join("..")
            .join("..")
            .join(b.path().file_name().unwrap());
        assert!(escape.join(".credentials.json").is_file());
        assert_eq!(outcome(&escape), REFUSED);
    }

    #[test]
    fn parent_dir_inside_own_fixture_is_refused() {
        let a = tempfile::tempdir().unwrap();
        seed(a.path());
        let dotted = a.path().join("home/../home/.claude");
        assert!(dotted.join(".credentials.json").is_file());
        assert_eq!(outcome(&dotted), REFUSED);
    }

    #[test]
    fn prefix_sibling_is_refused() {
        let a = tempfile::tempdir().unwrap();
        seed(a.path());
        let s = tempfile::Builder::new()
            .prefix(a.path().file_name().unwrap())
            .tempdir()
            .unwrap();
        assert_ne!(s.path(), a.path());
        assert!(
            s.path()
                .to_str()
                .unwrap()
                .starts_with(a.path().to_str().unwrap())
        );
        valid(s.path());
        assert_eq!(outcome(s.path()), REFUSED);
    }

    #[test]
    fn temp_dir_itself_is_refused_even_after_registration() {
        let a = tempfile::tempdir().unwrap();
        seed(a.path());
        assert_eq!(outcome(&std::env::temp_dir()), REFUSED);
    }

    #[test]
    fn relative_path_is_refused() {
        let a = tempfile::tempdir().unwrap();
        seed(a.path());
        let rel = a
            .path()
            .strip_prefix(std::env::temp_dir())
            .unwrap()
            .join("home/.claude");
        assert!(rel.is_relative());
        assert_eq!(outcome(&rel), REFUSED);
    }

    #[test]
    fn own_fixture_is_read() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path());
        assert_eq!(read(&t.path().join("home/.claude")), Ok(account("work")));
    }

    #[test]
    fn credential_moved_within_own_fixture_is_read() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path());
        let moved = t.path().join("root/work");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::rename(
            t.path().join("home/.claude/.credentials.json"),
            moved.join(".credentials.json"),
        )
        .unwrap();
        assert_eq!(read(&moved), Ok(account("work")));
    }

    #[test]
    fn fixture_root_itself_is_read() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path());
        valid(t.path());
        assert_eq!(read(t.path()), Ok(account("x")));
    }

    #[test]
    fn other_errors_are_unchanged_inside_own_fixture() {
        let t = tempfile::tempdir().unwrap();
        seed(t.path());
        let empty = t.path().join("root/none");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(outcome(&empty), Err("fake login missing"));
        std::fs::write(empty.join(".credentials.json"), "not json").unwrap();
        assert_eq!(outcome(&empty), Err("fake login invalid"));
        std::fs::write(empty.join(".credentials.json"), r#"{"claudeAiOauth":{}}"#).unwrap();
        assert_eq!(outcome(&empty), Err("fake identity missing"));
        std::fs::write(
            empty.join(".credentials.json"),
            r#"{"task1753FakeIdentity":{}}"#,
        )
        .unwrap();
        assert_eq!(outcome(&empty), Err("fake identity invalid"));
    }

    #[test]
    #[should_panic]
    fn body_refuses_a_file_directly_in_temp_dir() {
        let _ = body(&std::env::temp_dir().join(".credentials.json"));
    }

    #[test]
    #[should_panic]
    fn body_refuses_a_parent_dir_component() {
        let t = tempfile::tempdir().unwrap();
        let _ = body(&t.path().join("home/../.claude/.credentials.json"));
    }
}
