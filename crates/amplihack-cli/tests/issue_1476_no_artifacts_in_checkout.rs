//! crates/amplihack-cli/tests/issue_1476_no_artifacts_in_checkout.rs
//!
//! Issue #1476 / AC5' — the regression test for the reported failure.
//!
//! In `rysweet/amplihack-recipe-runner`, a routine `git add -A` staged an 8 MB
//! `.amplihack/graph_db` binary that amplihack had written into the user's
//! checkout. It only ever looked safe from inside `amplihack-rs`, whose own
//! `.gitignore` happens to list `index.scip` and `.amplihack/`; no other
//! indexed repository has those lines.
//!
//! So the assertion is the user's: index a throwaway git repo and
//! `git status --porcelain` must be **byte-empty** afterwards.
//!
//! This test is unconditional and needs no `scip-*` binary installed — it uses
//! shell stubs on a temporary PATH, following the existing harness in
//! `scip_indexing/mod.rs`. A test gated on a real indexer is a test that never
//! runs in CI, which is a test that does not exist.

use amplihack_memory::cli_memory::{project_artifact_root, run_native_scip_indexing};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: every test in this file holds `env_lock()`.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: every test in this file holds `env_lock()`.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: every test in this file holds `env_lock()`.
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("git output is utf-8")
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

/// A stub that refuses to fall back on the old convention. If the ladder ever
/// stops passing an explicit output path, this fails loudly instead of quietly
/// writing `index.scip` into the checkout and passing anyway.
const SCIP_STUB: &str = r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output" ] || [ "$prev" = "-o" ]; then
    out="$arg"
  fi
  prev="$arg"
done
if [ -z "$out" ]; then
  echo "stub: refusing to write index.scip by convention; no --output was passed" >&2
  exit 1
fi
mkdir -p "$(dirname "$out")"
printf 'stub-scip' > "$out"
"#;

/// A repository carrying one `.py` and one `.js` file. The `.js` file is
/// deliberate: it drives the `javascript` arm, which writes a `tsconfig.json`
/// into the project root, so the cleanliness assertion actually exercises it.
fn fixture_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(repo.path(), &["config", "user.email", "t@example.invalid"]);
    git(repo.path(), &["config", "user.name", "Issue 1476"]);
    fs::write(repo.path().join("app.py"), "print('hi')\n").unwrap();
    fs::write(repo.path().join("app.js"), "module.exports = {};\n").unwrap();
    fs::write(repo.path().join("README.md"), "# fixture\n").unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-qm", "initial"]);
    // No .gitignore: unlike amplihack-rs, a normal repository has no lines
    // hiding `.amplihack/` or `index.scip`.
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
    repo
}

fn stub_bin_dir() -> tempfile::TempDir {
    let bin = tempfile::tempdir().unwrap();
    for tool in [
        "scip-python",
        "scip-typescript",
        "scip-go",
        "rust-analyzer",
        "scip-dotnet",
        "scip-clang",
    ] {
        write_executable(&bin.path().join(tool), SCIP_STUB);
    }
    // Prerequisite checks also require these companions on PATH.
    for tool in ["node", "go", "cargo", "dotnet"] {
        write_executable(&bin.path().join(tool), "#!/bin/sh\nexit 0\n");
    }
    bin
}

#[test]
fn indexing_a_repository_leaves_its_working_tree_byte_clean() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let repo = fixture_repo();
    let bin = stub_bin_dir();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let _graph = EnvVarGuard::unset("AMPLIHACK_GRAPH_DB_PATH");
    let _kuzu = EnvVarGuard::unset("AMPLIHACK_KUZU_DB_PATH");
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let _path = EnvVarGuard::set("PATH", format!("{}:{inherited_path}", bin.path().display()));

    let summary = run_native_scip_indexing(Some(repo.path()), &[]).unwrap();

    assert!(summary.success, "indexing failed: {:?}", summary.errors);

    // The user's assertion.
    assert_eq!(
        git(repo.path(), &["status", "--porcelain"]),
        "",
        "indexing dirtied the working tree — this is the bug: a routine \
         `git add -A` in an indexed repository stages amplihack's artifacts"
    );

    // And the specific paths, named, so a failure says what landed.
    assert!(
        !repo.path().join(".amplihack").exists(),
        ".amplihack must not exist inside an indexed repository"
    );
    assert!(
        !repo.path().join("index.scip").exists(),
        "a bare index.scip must not be left in the repository root"
    );
    assert!(
        !repo.path().join("tsconfig.json").exists(),
        "the javascript arm's temporary tsconfig.json must be cleaned up"
    );
}

/// Clean is necessary but not sufficient: the artifacts must actually exist
/// somewhere. A "fix" that simply stops indexing would pass the test above.
#[test]
fn the_artifacts_exist_under_the_per_project_cache_root() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let repo = fixture_repo();
    let bin = stub_bin_dir();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let _path = EnvVarGuard::set("PATH", format!("{}:{inherited_path}", bin.path().display()));

    let summary = run_native_scip_indexing(Some(repo.path()), &[]).unwrap();

    let root = project_artifact_root(repo.path()).unwrap();
    assert!(
        root.starts_with(cache.path().join("amplihack").join("projects")),
        "artifact root {} is not under the XDG cache",
        root.display()
    );
    assert!(
        !summary.artifacts.is_empty(),
        "a clean checkout with no artifacts anywhere is not a fix"
    );
    for artifact in &summary.artifacts {
        assert!(
            artifact.starts_with(&root),
            "artifact {} is not under the project's artifact root",
            artifact.display()
        );
        assert_eq!(fs::read_to_string(artifact).unwrap(), "stub-scip");
    }
    assert_eq!(
        fs::read_to_string(root.join("indexes").join("python.scip")).unwrap(),
        "stub-scip"
    );
}

/// The same repository indexed twice must still be clean — the second run takes
/// the "artifact already exists" paths that the first one does not.
#[test]
fn a_second_indexing_run_also_leaves_the_tree_clean() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let repo = fixture_repo();
    let bin = stub_bin_dir();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let _path = EnvVarGuard::set("PATH", format!("{}:{inherited_path}", bin.path().display()));

    run_native_scip_indexing(Some(repo.path()), &[]).unwrap();
    run_native_scip_indexing(Some(repo.path()), &[]).unwrap();

    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
    assert!(!repo.path().join(".amplihack").exists());
}
