//! crates/amplihack-memory/src/cli_memory/artifact_root_tests.rs
//!
//! Issue #1476 — contracts for `cli_memory::artifact_root`.
//!
//! Wire up from `artifact_root.rs` with:
//! ```ignore
//! #[cfg(test)]
//! #[path = "artifact_root_tests.rs"]
//! mod tests;
//! ```
//!
//! The single invariant these tests exist to defend: **no artifact path this
//! module produces may live inside the project checkout**. Everything else here
//! (XDG precedence, slug shape, 0o700 chain, pointer file) is machinery in
//! service of that.
//!
//! Every test acquires `test_support::env_lock()` (D15/AC18) — `set_var` is
//! process-global and these tests mutate `HOME`, `XDG_CACHE_HOME`, and
//! `AMPLIHACK_ARTIFACT_DIR`.

use super::{
    ArtifactRootInit, ensure_artifact_root, project_artifact_root, project_for_artifact_dir,
    project_slug, validate_env_dir_path,
};
use crate::test_support::{HomeGuard, env_lock};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// RAII guard for an arbitrary environment variable. `env_lock()` must already
/// be held by the caller.
struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: serialised by env_lock(); restored on drop.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: serialised by env_lock(); restored on drop.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: serialised by env_lock().
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Standard fixture: a project dir, an isolated HOME, an isolated XDG cache,
/// and no `AMPLIHACK_ARTIFACT_DIR`.
struct Fixture {
    project: TempDir,
    home: TempDir,
    cache: TempDir,
    _home_guard: HomeGuard,
    _xdg: EnvVarGuard,
    _override: EnvVarGuard,
}

fn fixture() -> Fixture {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let _home_guard = HomeGuard::set(home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _override = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    Fixture {
        project,
        home,
        cache,
        _home_guard,
        _xdg,
        _override,
    }
}

// ── AC2 / AC3: resolution order ───────────────────────────────────────────

/// AC2 — the load-bearing assertion. The artifact root is never inside the
/// project. This is the bug from `rysweet/amplihack-recipe-runner`, stated once
/// in its simplest form.
#[test]
fn artifact_root_is_never_inside_the_project_checkout() {
    let _g = lock();
    let fx = fixture();

    let root = project_artifact_root(fx.project.path()).unwrap();

    assert!(
        !root.starts_with(fx.project.path()),
        "artifact root {} must not be inside the project {}",
        root.display(),
        fx.project.path().display()
    );
    assert!(root.is_absolute(), "artifact root must be absolute");
}

/// AC3 — `XDG_CACHE_HOME` is preferred, under `amplihack/projects/<slug>`.
#[test]
fn artifact_root_prefers_xdg_cache_home() {
    let _g = lock();
    let fx = fixture();

    let root = project_artifact_root(fx.project.path()).unwrap();

    let expected_parent = fx.cache.path().join("amplihack").join("projects");
    assert_eq!(
        root.parent(),
        Some(expected_parent.as_path()),
        "artifact root must be <XDG_CACHE_HOME>/amplihack/projects/<slug>, got {}",
        root.display()
    );
    assert_eq!(
        root.file_name().and_then(|n| n.to_str()),
        Some(project_slug(fx.project.path()).as_str())
    );
}

/// AC3 — `HOME/.cache` is the fallback when `XDG_CACHE_HOME` is unset.
#[test]
fn artifact_root_falls_back_to_home_cache_when_xdg_unset() {
    let _g = lock();
    let fx = fixture();
    let _xdg = EnvVarGuard::unset("XDG_CACHE_HOME");

    let root = project_artifact_root(fx.project.path()).unwrap();

    assert_eq!(
        root.parent(),
        Some(
            fx.home
                .path()
                .join(".cache")
                .join("amplihack")
                .join("projects")
                .as_path()
        )
    );
}

/// D7 — a relative `XDG_CACHE_HOME` is invalid per the XDG spec: ignore it and
/// fall through to `$HOME/.cache`. It must never be joined against the cwd,
/// which is how artifacts land in whatever repo the process is standing in.
#[test]
fn artifact_root_ignores_relative_xdg_cache_home() {
    let _g = lock();
    let fx = fixture();
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", "relative/cache");

    let root = project_artifact_root(fx.project.path()).unwrap();

    assert!(
        root.starts_with(fx.home.path().join(".cache")),
        "relative XDG_CACHE_HOME must fall through to $HOME/.cache, got {}",
        root.display()
    );
}

/// D7 — an empty `XDG_CACHE_HOME` is treated as unset.
#[test]
fn artifact_root_ignores_empty_xdg_cache_home() {
    let _g = lock();
    let fx = fixture();
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", "");

    let root = project_artifact_root(fx.project.path()).unwrap();

    assert!(root.starts_with(fx.home.path().join(".cache")));
}

/// D7 — no HOME and no XDG is an error, never a cwd fallback.
#[test]
fn artifact_root_errors_when_home_and_xdg_are_both_unset() {
    let _g = lock();
    let project = TempDir::new().unwrap();
    let _home = EnvVarGuard::unset("HOME");
    let _xdg = EnvVarGuard::unset("XDG_CACHE_HOME");
    let _override = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");

    let err = project_artifact_root(project.path())
        .expect_err("unresolvable artifact root must be an error, not a cwd fallback");

    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("HOME") || rendered.contains("XDG_CACHE_HOME"),
        "error must name the variable the user can set; got: {rendered}"
    );
}

// ── AC4 / SEC-1 / SEC-2: AMPLIHACK_ARTIFACT_DIR override ──────────────────

/// AC4 — the override mirrors `AMPLIHACK_GRAPH_DB_PATH`: it names the directory
/// directly and wins over XDG.
#[test]
fn artifact_dir_override_wins_over_xdg() {
    let _g = lock();
    let fx = fixture();
    let explicit = TempDir::new().unwrap();
    let _override = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", explicit.path());

    let root = project_artifact_root(fx.project.path()).unwrap();

    assert_eq!(root, explicit.path());
}

/// SEC-1 — a relative override is rejected outright. The user asked for that
/// specific path, so unlike XDG this is an error rather than a fall-through.
#[test]
fn artifact_dir_override_rejects_relative_path() {
    let _g = lock();
    let fx = fixture();
    let _override = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", "cache/amplihack");

    let err = project_artifact_root(fx.project.path()).unwrap_err();

    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("AMPLIHACK_ARTIFACT_DIR"),
        "error must name AMPLIHACK_ARTIFACT_DIR, not the subsystem that read it; got: {rendered}"
    );
}

#[test]
fn artifact_dir_override_rejects_parent_traversal() {
    let _g = lock();
    let fx = fixture();
    let _override = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", "/var/cache/../../etc/amplihack");

    let err = project_artifact_root(fx.project.path()).unwrap_err();

    assert!(format!("{err:#}").contains("AMPLIHACK_ARTIFACT_DIR"));
}

/// SEC-2 — `/`, `$HOME`, `/tmp` and `/var/tmp` are rejected as the artifact
/// root. `enforce_db_permissions` chmods the artifact root's chain; pointing it
/// at `$HOME` would chmod the user's home directory, and `/tmp` would break
/// every other user on the host.
#[test]
fn artifact_dir_override_rejects_dangerous_roots() {
    let _g = lock();
    let fx = fixture();
    let home_value = fx.home.path().to_path_buf();

    for candidate in [
        PathBuf::from("/"),
        home_value,
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        PathBuf::from("/proc/self"),
        PathBuf::from("/sys/kernel"),
        PathBuf::from("/dev/shm"),
    ] {
        let _override = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", &candidate);
        let result = project_artifact_root(fx.project.path());
        assert!(
            result.is_err(),
            "AMPLIHACK_ARTIFACT_DIR={} must be rejected, got {:?}",
            candidate.display(),
            result
        );
    }
}

/// SEC-1 — one validator, parameterised by variable name, so the message always
/// names the variable the user actually set.
#[test]
fn validate_env_dir_path_names_the_variable_in_every_rejection() {
    for (var, path) in [
        ("XDG_CACHE_HOME", "relative/path"),
        ("AMPLIHACK_ARTIFACT_DIR", "/a/../b"),
        ("HOME", "/proc/self"),
    ] {
        let result = validate_env_dir_path(var, Path::new(path));
        assert!(result.is_err(), "{var}={path} must be rejected");
        let rendered = format!("{:#}", result.unwrap_err());
        assert!(
            rendered.contains(var),
            "rejection for {var}={path} must name {var}; got: {rendered}"
        );
    }
}

// ── D9: slug shape ────────────────────────────────────────────────────────

/// D9 — the hash, not the readable part, carries uniqueness. Two checkouts with
/// the same basename (the normal case for git worktrees in this repo) must not
/// share an artifact directory.
#[test]
fn slug_distinguishes_same_basename_in_different_parents() {
    let parent_a = TempDir::new().unwrap();
    let parent_b = TempDir::new().unwrap();
    let a = parent_a.path().join("amplihack-rs");
    let b = parent_b.path().join("amplihack-rs");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();

    assert_ne!(project_slug(&a), project_slug(&b));
    assert!(project_slug(&a).starts_with("amplihack-rs-"));
    assert!(project_slug(&b).starts_with("amplihack-rs-"));
}

#[test]
fn slug_is_stable_across_calls_and_trailing_separators() {
    let project = TempDir::new().unwrap();
    let with_slash = PathBuf::from(format!("{}/", project.path().display()));

    let first = project_slug(project.path());
    assert_eq!(first, project_slug(project.path()));
    assert_eq!(first, project_slug(&with_slash));
}

/// A2 — resolution is keyed on the canonical absolute path, so a symlink to the
/// project and the project itself are one identity, not two caches.
#[test]
#[cfg(unix)]
fn slug_canonicalises_symlinked_project_paths() {
    let real = TempDir::new().unwrap();
    let link_parent = TempDir::new().unwrap();
    let link = link_parent.path().join("link-to-project");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();

    assert_eq!(project_slug(real.path()), project_slug(&link));
}

#[test]
fn slug_sanitises_the_human_readable_part_and_keeps_a_16_hex_suffix() {
    let parent = TempDir::new().unwrap();
    let odd = parent.path().join("my project (v2)!");
    fs::create_dir_all(&odd).unwrap();

    let slug = project_slug(&odd);

    assert!(
        slug.starts_with("my-project-v2-"),
        "expected sanitised basename prefix, got {slug}"
    );
    let hash = &slug[slug.len() - 16..];
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
        "slug must end in 16 lowercase hex chars, got {slug}"
    );
    assert!(
        !slug.contains('/') && !slug.contains(std::path::MAIN_SEPARATOR),
        "slug must be a single path component, got {slug}"
    );
}

/// D9 — a basename that sanitises away entirely still yields a usable slug.
#[test]
fn slug_falls_back_to_project_when_the_basename_sanitises_to_nothing() {
    let parent = TempDir::new().unwrap();
    let odd = parent.path().join("!!!");
    fs::create_dir_all(&odd).unwrap();

    assert!(project_slug(&odd).starts_with("project-"));
}

/// D9 — the human part is truncated, the hash is not.
#[test]
fn slug_truncates_a_very_long_basename_but_keeps_the_hash() {
    let parent = TempDir::new().unwrap();
    // 200, not 300: Linux caps a single path component at NAME_MAX (255 bytes),
    // so `create_dir_all` on a 300-character name fails with ENAMETOOLONG before
    // `project_slug` is ever called — the test failed in its own setup and looked
    // like a truncation bug. 200 is still far past SLUG_HUMAN_MAX (48), so it
    // exercises the truncation this test exists to prove.
    let long = parent.path().join("a".repeat(200));
    fs::create_dir_all(&long).unwrap();

    let slug = project_slug(&long);

    assert!(slug.len() <= 48 + 1 + 16, "slug too long: {}", slug.len());
    assert!(
        slug[slug.len() - 16..]
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    );
}

// ── SEC-3 / R19: directory permissions ────────────────────────────────────

/// SEC-3 — the whole chain is created at 0o700, never at umask width even
/// briefly, and afterwards verified.
#[test]
#[cfg(unix)]
fn ensure_artifact_root_creates_the_whole_chain_at_0700() {
    let _g = lock();
    let fx = fixture();

    let init = ensure_artifact_root(fx.project.path()).unwrap();

    assert!(init.created, "first call must report it created the root");
    let mut dir = Some(init.root.as_path());
    while let Some(current) = dir {
        if current == fx.cache.path() {
            break;
        }
        let mode = fs::symlink_metadata(current).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o077,
            0,
            "{} is group/world accessible (mode {:o})",
            current.display(),
            mode
        );
        dir = current.parent();
    }
}

/// R19 / SEC-10 — `create_dir_all` does not re-tighten a pre-existing loose
/// directory, so a `~/.cache/amplihack` left at 0o755 by an earlier version
/// must be repaired, not merely tolerated.
#[test]
#[cfg(unix)]
fn ensure_artifact_root_repairs_a_pre_existing_loose_mode() {
    let _g = lock();
    let fx = fixture();
    let loose = fx.cache.path().join("amplihack");
    fs::create_dir_all(&loose).unwrap();
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();

    ensure_artifact_root(fx.project.path()).unwrap();

    let mode = fs::symlink_metadata(&loose).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "loose mode not repaired: {mode:o}");
}

#[test]
fn ensure_artifact_root_is_idempotent_and_reports_created_only_once() {
    let _g = lock();
    let fx = fixture();

    let first = ensure_artifact_root(fx.project.path()).unwrap();
    let second = ensure_artifact_root(fx.project.path()).unwrap();

    assert_eq!(first.root, second.root);
    assert!(first.created);
    assert!(
        !second.created,
        "`created` drives whether enforce_db_permissions may chmod; it must not \
         re-report true for a directory that already existed"
    );
}

/// SEC-2 — when the root came from an env override, amplihack did not create it
/// and must not claim it did.
#[test]
fn ensure_artifact_root_reports_not_created_for_a_pre_existing_override_dir() {
    let _g = lock();
    let fx = fixture();
    let explicit = TempDir::new().unwrap();
    let _override = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", explicit.path());

    let init: ArtifactRootInit = ensure_artifact_root(fx.project.path()).unwrap();

    assert_eq!(init.root, explicit.path());
    assert!(
        !init.created,
        "amplihack must not chmod a directory the user pointed it at"
    );
}

// ── D4 / SEC-6 / SEC-7: the pointer file ──────────────────────────────────

/// AC15 — the pointer file is the reverse mapping. Without it,
/// `project_root_for_blarify_input` silently resolves to the cwd.
#[test]
fn pointer_file_round_trips_the_canonical_project_path() {
    let _g = lock();
    let fx = fixture();

    let init = ensure_artifact_root(fx.project.path()).unwrap();
    let recovered = project_for_artifact_dir(&init.root).unwrap();

    assert_eq!(
        recovered,
        Some(fx.project.path().canonicalize().unwrap()),
        "pointer must record the canonical project path"
    );
}

#[test]
fn pointer_file_is_absent_before_the_root_is_ensured() {
    let _g = lock();
    let fx = fixture();
    let root = project_artifact_root(fx.project.path()).unwrap();
    fs::create_dir_all(&root).unwrap();

    assert_eq!(project_for_artifact_dir(&root).unwrap(), None);
}

/// SEC-7 — the pointer is a 0o600 regular file.
#[test]
#[cfg(unix)]
fn pointer_file_is_written_0600() {
    let _g = lock();
    let fx = fixture();

    let init = ensure_artifact_root(fx.project.path()).unwrap();

    let pointer = init.root.join("project");
    let meta = fs::symlink_metadata(&pointer).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

/// SEC-7 — no temp file survives a successful write.
#[test]
fn pointer_write_leaves_no_temp_files_behind() {
    let _g = lock();
    let fx = fixture();

    let init = ensure_artifact_root(fx.project.path()).unwrap();

    let leftovers: Vec<_> = fs::read_dir(&init.root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "project")
        .collect();
    assert!(
        leftovers.is_empty(),
        "unexpected entries after ensure_artifact_root: {leftovers:?}"
    );
}

/// SEC-6 — the pointer is untrusted input that decides a write path. Oversized,
/// relative, traversing, NUL-bearing and non-regular pointers are all rejected.
#[test]
fn pointer_file_rejects_malformed_contents() {
    let _g = lock();
    let fx = fixture();
    let init = ensure_artifact_root(fx.project.path()).unwrap();
    let pointer = init.root.join("project");

    for bad in [
        "relative/project".to_string(),
        "/tmp/../etc/project".to_string(),
        "/tmp/pro\0ject".to_string(),
        "x".repeat(8 * 1024),
    ] {
        fs::write(&pointer, &bad).unwrap();
        assert!(
            project_for_artifact_dir(&init.root).is_err(),
            "pointer contents {:?} must be rejected",
            &bad[..bad.len().min(40)]
        );
    }
}

#[test]
#[cfg(unix)]
fn pointer_file_rejects_a_symlink() {
    let _g = lock();
    let fx = fixture();
    let init = ensure_artifact_root(fx.project.path()).unwrap();
    let pointer = init.root.join("project");
    let elsewhere = fx.home.path().join("planted");
    fs::write(&elsewhere, fx.project.path().to_str().unwrap()).unwrap();
    fs::remove_file(&pointer).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &pointer).unwrap();

    assert!(
        project_for_artifact_dir(&init.root).is_err(),
        "a symlinked pointer must be refused, not followed"
    );
}

// ── D3 / AC14: inherited override across projects ─────────────────────────

/// AC14 — `env_builder` exports `AMPLIHACK_ARTIFACT_DIR` to every child, and
/// this repo runs agents in git worktrees. A child launched for project B must
/// not write B's index into project A's cache.
#[test]
fn inherited_override_naming_another_project_is_ignored_with_a_warning() {
    let _g = lock();
    let fx = fixture();
    let project_b = TempDir::new().unwrap();

    // Project A's artifact dir, complete with its pointer file.
    let a_root = ensure_artifact_root(fx.project.path()).unwrap().root;
    let _override = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", &a_root);

    let b_root = project_artifact_root(project_b.path()).unwrap();

    assert_ne!(
        b_root, a_root,
        "project B must not inherit project A's artifact dir"
    );
    assert_eq!(
        b_root.parent(),
        Some(fx.cache.path().join("amplihack").join("projects").as_path()),
        "mismatched override must fall through to XDG, never to the named directory"
    );
}

/// The complement: an override whose pointer names *this* project is honoured.
#[test]
fn inherited_override_naming_this_project_is_honoured() {
    let _g = lock();
    let fx = fixture();

    let a_root = ensure_artifact_root(fx.project.path()).unwrap().root;
    let _override = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", &a_root);

    assert_eq!(project_artifact_root(fx.project.path()).unwrap(), a_root);
}
