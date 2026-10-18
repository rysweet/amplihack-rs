//! Contracts for `scripts/check-brick-budget.sh` (PR #1530).
//!
//! The hook reads `BRICK_LIMIT` and `PHASE_RECIPES` out of
//! `tests/integration/default_workflow_decomposition_test.rs` with sed. It used
//! GNU-only `\+`, which BSD sed on macOS reads as a literal plus: the limit came
//! back empty and the hook failed every commit that staged a recipe.
//!
//! CI runs on Linux with GNU sed, so these tests force BSD-like semantics by
//! putting a `sed` shim on PATH that runs GNU sed with `--posix`. Under
//! `--posix`, GNU sed treats `\+` the same way BSD sed does. On a host whose sed
//! has no `--posix` (macOS), the real sed is already BSD and no shim is needed.
//!
//! Every hook run happens in a temporary copy of the repository files the hook
//! reads, outside any git repository, so the hook falls back to `pwd` as its
//! root and the real recipes are never touched.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const HOOK: &str = "scripts/check-brick-budget.sh";
const DECOMP_TEST: &str = "tests/integration/default_workflow_decomposition_test.rs";
const RECIPE_DIR: &str = "amplifier-bundle/recipes";

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Phase recipe names exactly as the Rust authority declares them.
fn phase_recipes_from_authority() -> Vec<String> {
    let text = read(&workspace_root().join(DECOMP_TEST));
    let start = text
        .find("const PHASE_RECIPES: &[&str] = &[")
        .expect("PHASE_RECIPES const must exist in the decomposition test");
    let body = &text[start..];
    let end = body
        .find("\n];")
        .expect("PHASE_RECIPES must close with `];`");
    body[..end]
        .lines()
        .skip(1)
        .filter_map(|l| {
            l.trim()
                .strip_prefix('"')?
                .split('"')
                .next()
                .map(str::to_owned)
        })
        .collect()
}

/// A sed on PATH that behaves like BSD sed on `\+`, or `None` when the host
/// sed has no `--posix` (it is then BSD sed already).
fn posix_sed_shim(dir: &Path) -> Option<PathBuf> {
    let probe = Command::new("sed")
        .args(["--posix", "-n", "p"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !probe.status.success() {
        return None;
    }
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v sed"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let bin = dir.join("posix-bin");
    let shim = bin.join("sed");
    write(
        &shim,
        &format!("#!/bin/sh\nexec {} --posix \"$@\"\n", real.trim()),
    );
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    Some(bin)
}

/// A throwaway tree holding the hook, its source of truth and the recipes.
struct Fixture {
    tmp: TempDir,
}

impl Fixture {
    /// Copy the real hook, the real decomposition test and every recipe the
    /// hook checks.
    fn from_repo() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = workspace_root();
        let f = Fixture { tmp };
        write(&f.path(HOOK), &read(&root.join(HOOK)));
        write(&f.path(DECOMP_TEST), &read(&root.join(DECOMP_TEST)));
        for name in phase_recipes_from_authority()
            .into_iter()
            .chain(["default-workflow".to_owned()])
        {
            let rel = format!("{RECIPE_DIR}/{name}.yaml");
            write(&f.path(&rel), &read(&root.join(&rel)));
        }
        f
    }

    /// The hook plus a synthetic decomposition test and recipe set.
    fn synthetic(limit_line: &str, recipes: &[(&str, usize)]) -> Self {
        let tmp = TempDir::new().unwrap();
        let f = Fixture { tmp };
        write(&f.path(HOOK), &read(&workspace_root().join(HOOK)));
        let mut src = format!("{limit_line}\n\nconst PHASE_RECIPES: &[&str] = &[\n");
        for (name, _) in recipes.iter().filter(|(n, _)| *n != "default-workflow") {
            src.push_str(&format!("    \"{name}\",\n"));
        }
        src.push_str("];\n");
        write(&f.path(DECOMP_TEST), &src);
        for (name, lines) in recipes {
            write(
                &f.path(&format!("{RECIPE_DIR}/{name}.yaml")),
                &"x: 1\n".repeat(*lines),
            );
        }
        f
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn set_decomp_test(&self, text: &str) {
        write(&self.path(DECOMP_TEST), text);
    }

    /// Run the hook from the fixture root, outside any git repository.
    fn run(&self, posix_sed: bool) -> Output {
        let mut cmd = Command::new("bash");
        cmd.arg(HOOK)
            .current_dir(self.tmp.path())
            .env("GIT_CEILING_DIRECTORIES", self.tmp.path().parent().unwrap())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        if posix_sed && let Some(bin) = posix_sed_shim(self.tmp.path()) {
            let path = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{}:{path}", bin.display()));
        }
        cmd.output().expect("run check-brick-budget.sh")
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ---------------------------------------------------------------- static ---

/// No sed expression in the hook may use GNU-only BRE operators. Comment
/// lines are skipped: the hook explains in a comment why `\+` is avoided.
#[test]
fn hook_sed_expressions_use_posix_bre_only() {
    let text = read(&workspace_root().join(HOOK));
    let offenders: Vec<_> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with('#') && l.contains("sed "))
        .filter(|(_, l)| ["\\+", "\\?", "\\|"].iter().any(|op| l.contains(op)))
        .map(|(i, l)| format!("{}:{}: {}", HOOK, i + 1, l.trim()))
        .collect();
    assert!(
        offenders.is_empty(),
        "GNU-only BRE operator in a sed expression; BSD sed on macOS reads it \
         literally. Use `[x][x]*` instead of `x\\+`:\n{}",
        offenders.join("\n")
    );
}

// ----------------------------------------------------------- real inputs ---

#[test]
fn hook_passes_on_repository_recipes_with_host_sed() {
    let out = Fixture::from_repo().run(false);
    assert!(
        out.status.success(),
        "hook failed on the repo's recipes:\n{}",
        stderr(&out)
    );
}

/// The regression PR #1530 fixes: with BSD sed semantics the old `\+` form
/// read no limit and exited 1 with "could not read BRICK_LIMIT".
#[test]
fn hook_passes_on_repository_recipes_with_posix_sed() {
    let out = Fixture::from_repo().run(true);
    assert!(
        out.status.success(),
        "hook failed under POSIX/BSD sed semantics (GNU-only regex?):\n{}",
        stderr(&out)
    );
}

/// The hook must read the same limit as the Rust authority under POSIX sed:
/// lowering only the limit has to turn the pass into a violation.
#[test]
fn posix_sed_reads_the_limit_from_the_authority() {
    let f = Fixture::from_repo();
    let src = read(&f.path(DECOMP_TEST));
    assert!(
        src.contains("const BRICK_LIMIT: usize = 400;"),
        "authority limit moved"
    );
    f.set_decomp_test(&src.replace(
        "const BRICK_LIMIT: usize = 400;",
        "const BRICK_LIMIT: usize = 1;",
    ));
    let out = f.run(true);
    assert_eq!(
        out.status.code(),
        Some(1),
        "limit 1 must fail:\n{}",
        stderr(&out)
    );
    let err = stderr(&out);
    assert!(
        err.contains("brick rule violation"),
        "expected a violation report:\n{err}"
    );
    // Every phase recipe plus default-workflow must be checked, i.e. the
    // recipe list was parsed in full under POSIX sed.
    for name in phase_recipes_from_authority()
        .iter()
        .map(String::as_str)
        .chain(["default-workflow"])
    {
        assert!(
            err.contains(&format!("amplifier-bundle/recipes/{name}.yaml")),
            "recipe `{name}` was not checked under POSIX sed:\n{err}"
        );
    }
}

// --------------------------------------------------------- limit parsing ---

/// A multi-digit limit must be read whole, not just its last digit.
#[test]
fn multi_digit_limit_is_read_whole_under_posix_sed() {
    let f = Fixture::synthetic(
        "const BRICK_LIMIT: usize = 1234;",
        &[("phase-a", 1233), ("default-workflow", 1)],
    );
    let out = f.run(true);
    assert!(
        out.status.success(),
        "1233 < 1234 must pass:\n{}",
        stderr(&out)
    );
}

// ----------------------------------------------------------- fail closed ---

#[test]
fn missing_limit_fails_closed() {
    let f = Fixture::synthetic(
        "// no limit here",
        &[("phase-a", 1), ("default-workflow", 1)],
    );
    let out = f.run(true);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("could not read BRICK_LIMIT"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn non_numeric_limit_fails_closed() {
    let f = Fixture::synthetic(
        "const BRICK_LIMIT: usize = LIMIT;",
        &[("phase-a", 1), ("default-workflow", 1)],
    );
    let out = f.run(true);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("could not read BRICK_LIMIT"),
        "{}",
        stderr(&out)
    );
}

// ------------------------------------------- macOS clippy regressions ---
//
// CI's macOS jobs build release binaries only and never run clippy
// --all-targets, so these static checks are the only guard against the
// PR #1530 fixes being reverted on Linux, where both forms compile cleanly.

#[test]
fn file_lock_casts_f_wrlck_to_c_short() {
    let text = read(&workspace_root().join("crates/amplihack-state/src/file_lock.rs"));
    assert!(
        text.contains("F_WRLCK as libc::c_short"),
        "cast F_WRLCK to libc::c_short; `as i16` trips clippy unnecessary_cast on macOS"
    );
    assert!(
        !text.contains("F_WRLCK as i16"),
        "`F_WRLCK as i16` fails macOS clippy"
    );
}

#[test]
fn openpty_gets_mut_null_pointers() {
    let text = read(&workspace_root().join("tests/integration/no_python_probe_test.rs"));
    // From the call to the end of its `unsafe { ... };` block: cutting at the
    // first `)` would stop inside the first `null_mut()`.
    let call = text
        .find("openpty(")
        .map(|i| &text[i..i + text[i..].find("};").expect("openpty call must end")])
        .expect("no_python_probe_test must call openpty");
    assert!(
        call.contains("null_mut()"),
        "expected null_mut() arguments:\n{call}"
    );
    assert!(
        !call.contains("std::ptr::null()"),
        "openpty termp/winp are *mut on macOS; pass std::ptr::null_mut():\n{call}"
    );
}
