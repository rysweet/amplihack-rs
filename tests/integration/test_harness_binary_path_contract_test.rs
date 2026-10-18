//! TDD tests (issue #744): integration-test binary-path robustness contract.
//!
//! Several integration tests locate the compiled binary by hand-walking
//! `CARGO_MANIFEST_DIR` up to the workspace root and appending
//! `target/debug/amplihack`. That is fragile:
//!
//!   * it panics with "amplihack binary not found at target/debug/amplihack"
//!     when a prior `cargo build` was disrupted (an observed transient), and
//!   * it ignores `CARGO_TARGET_DIR` and the release/debug distinction.
//!
//! Cargo already exposes the correct, build-ordered path via the
//! `CARGO_BIN_EXE_<name>` environment variable, but *only* for test targets
//! that belong to the package producing that binary. So the fix is twofold:
//!
//!   1. Every affected test must resolve the binary via
//!      `env!("CARGO_BIN_EXE_amplihack")` / `env!("CARGO_BIN_EXE_amplihack-hooks")`
//!      and must NOT hand-roll `target/debug/<bin>`.
//!   2. `skip_update_check_flag_test.rs` — originally an orphan file not wired
//!      as a `[[test]]` — must be compiled into a `bins/amplihack` test target
//!      so it actually compiles/runs AND so `CARGO_BIN_EXE_amplihack` resolves
//!      for it. Since all integration tests became modules of the single
//!      `integration` target, that means a `mod` line in
//!      `tests/integration/main.rs`.
//!
//! Using `env!` (compile-time) rather than `option_env!`/`var_os` (soft, with a
//! hardcoded fallback) also forces the binary to be built as a test
//! prerequisite, eliminating the "binary not found" race entirely.
//!
//! ## Failure modes (RED until the harness fix lands)
//! Each assertion FAILS against the current fragile helpers and PASSES once the
//! path helpers are converted and the orphan test is wired.
//!
//! ## Issue #1378
//! Two more tests were carrying the same defect from
//! `crates/amplihack-cli/tests/`, where `CARGO_BIN_EXE_amplihack` does not
//! exist because that package does not produce the binary. They have been
//! moved into `bins/amplihack/tests/` so the Cargo-provided path resolves and
//! the binary is built as a test prerequisite; they are covered by the lists
//! below, which now hold workspace-relative paths rather than bare file names.
//!
//! ## Related
//! - `tests/integration/ci_speedup_optimization_test.rs`
//! - `docs/reference/ci-pipeline.md`

use std::path::PathBuf;

/// (workspace-relative test file path, binary name whose `CARGO_BIN_EXE_<name>`
/// it must use).
const AMPLIHACK_BIN: &str = "amplihack";
const HOOKS_BIN: &str = "amplihack-hooks";

const FRAGILE_TESTS: &[(&str, &str)] = &[
    ("tests/integration/cli_golden_tests.rs", AMPLIHACK_BIN),
    ("tests/integration/kuzu_path_notice_test.rs", AMPLIHACK_BIN),
    ("tests/integration/cli_launch_test.rs", AMPLIHACK_BIN),
    ("tests/integration/recipe_e2e_test.rs", AMPLIHACK_BIN),
    ("tests/integration/security_hygiene_test.rs", AMPLIHACK_BIN),
    ("tests/integration/fleet_probe.rs", AMPLIHACK_BIN),
    (
        "tests/integration/skip_update_check_flag_test.rs",
        AMPLIHACK_BIN,
    ),
    ("tests/integration/no_python_probe_test.rs", AMPLIHACK_BIN),
    (
        "tests/integration/doctor_node_remediation_test.rs",
        AMPLIHACK_BIN,
    ),
    ("tests/integration/hook_dispatch_test.rs", HOOKS_BIN),
    // Issue #1378: these two used to live in `crates/amplihack-cli/tests/`,
    // where `CARGO_BIN_EXE_amplihack` does not exist, and hand-rolled
    // `<workspace>/target/debug/amplihack` — which is wrong under any
    // redirected `CARGO_TARGET_DIR`. Moving them into the binary's own package
    // makes the Cargo-provided path available and builds the binary as a test
    // prerequisite.
    (
        "bins/amplihack/tests/issue_538_install_completeness.rs",
        AMPLIHACK_BIN,
    ),
    (
        "bins/amplihack/tests/issue_625_update_prompt_subprocess_safe.rs",
        AMPLIHACK_BIN,
    ),
];

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack → bins
    path.pop(); // bins → workspace root
    path
}

fn test_source_path(rel: &str) -> PathBuf {
    workspace_root().join(rel)
}

fn read_test_source(rel: &str) -> String {
    let p = test_source_path(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("FAIL: cannot read {p:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Guard: the files we are asserting about exist
// ---------------------------------------------------------------------------

#[test]
fn all_target_test_files_exist() {
    for (rel, _) in FRAGILE_TESTS {
        let p = test_source_path(rel);
        assert!(p.exists(), "FAIL: expected integration test {p:?} to exist");
    }
}

// ---------------------------------------------------------------------------
// 1. Each affected test must resolve the binary via CARGO_BIN_EXE_<name>
// ---------------------------------------------------------------------------

#[test]
fn fragile_tests_resolve_binary_via_cargo_bin_exe() {
    let mut offenders = Vec::new();
    for (file, bin) in FRAGILE_TESTS {
        let src = read_test_source(file);
        let needle = format!("env!(\"CARGO_BIN_EXE_{bin}\")");
        if !src.contains(&needle) {
            offenders.push(format!("  {file}: missing `{needle}`"));
        }
    }
    assert!(
        offenders.is_empty(),
        "FAIL: these integration tests must locate the binary via the Cargo-provided\n\
         env var instead of a hand-walked path:\n{}",
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 2. No hand-rolled target/debug/<bin> path push remains
// ---------------------------------------------------------------------------

#[test]
fn fragile_tests_drop_hardcoded_target_debug_path() {
    let mut offenders = Vec::new();
    for (file, bin) in FRAGILE_TESTS {
        let src = read_test_source(file);
        let bad = format!("push(\"target/debug/{bin}\")");
        if src.contains(&bad) {
            offenders.push(format!("  {file}: still contains `{bad}`"));
        }
    }
    assert!(
        offenders.is_empty(),
        "FAIL: these tests still hand-roll a `target/debug/<bin>` path, which\n\
         panics with 'binary not found' when a build is disrupted and ignores\n\
         CARGO_TARGET_DIR:\n{}",
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 3. The two "soft" helpers must become hard `env!` (no silent fallback)
// ---------------------------------------------------------------------------

#[test]
fn soft_lookup_tests_are_tightened_to_hard_env() {
    let no_python = read_test_source("tests/integration/no_python_probe_test.rs");
    assert!(
        !no_python.contains("option_env!(\"CARGO_BIN_EXE_amplihack\")"),
        "FAIL: no_python_probe_test.rs must use the hard `env!(\"CARGO_BIN_EXE_amplihack\")`,\n\
         not `option_env!` with a hardcoded fallback (which re-introduces the race)."
    );

    let doctor = read_test_source("tests/integration/doctor_node_remediation_test.rs");
    assert!(
        !doctor.contains("var_os(\"CARGO_BIN_EXE_amplihack\")"),
        "FAIL: doctor_node_remediation_test.rs must use the hard\n\
         `env!(\"CARGO_BIN_EXE_amplihack\")`, not `std::env::var_os(...)` with a\n\
         hardcoded `target/debug` fallback."
    );
}

// ---------------------------------------------------------------------------
// 4. skip_update_check_flag_test.rs must be compiled into a bins/amplihack
//    test target
// ---------------------------------------------------------------------------
//
// All `tests/integration/*.rs` files are modules of ONE test binary: the
// `integration` [[test]] target of bins/amplihack, rooted at
// `tests/integration/main.rs`. So "wired" now means two things, both of which
// must hold for the file to compile, run in CI, and see
// `CARGO_BIN_EXE_amplihack`:
//   (a) the manifest declares the `integration` target with that root, and
//   (b) the root declares skip_update_check_flag_test.rs as a module.

#[test]
fn skip_update_check_flag_test_is_wired_into_bin_package() {
    let mut cargo = workspace_root();
    cargo.push("bins");
    cargo.push("amplihack");
    cargo.push("Cargo.toml");
    let manifest = std::fs::read_to_string(&cargo)
        .unwrap_or_else(|e| panic!("FAIL: cannot read {cargo:?}: {e}"));
    assert!(
        manifest.contains(
            "[[test]]\nname = \"integration\"\npath = \"../../tests/integration/main.rs\"\n"
        ),
        "FAIL: bins/amplihack/Cargo.toml must declare the single integration\n\
         test binary that every tests/integration/*.rs file is compiled into:\n\
         \n\
           [[test]]\n\
           name = \"integration\"\n\
           path = \"../../tests/integration/main.rs\"\n"
    );

    let root = read_test_source("tests/integration/main.rs");
    let wired = root
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| {
            w[0] == "#[path = \"skip_update_check_flag_test.rs\"]"
                && w[1].starts_with("mod ")
                && w[1].ends_with(';')
        });
    assert!(
        wired,
        "FAIL: skip_update_check_flag_test.rs is an orphan file. It is not\n\
         declared as a module in tests/integration/main.rs (the root of the\n\
         `integration` test binary), so it never compiles/runs in CI and\n\
         `CARGO_BIN_EXE_amplihack` is unavailable to it.\n\
         Add to tests/integration/main.rs:\n\
         \n\
           #[path = \"skip_update_check_flag_test.rs\"]\n\
           mod skip_update_check_flag;\n"
    );
}

// ---------------------------------------------------------------------------
// Guard: no test file is left out of every test binary
// ---------------------------------------------------------------------------
//
// `tests/integration/` and `bins/amplihack/tests/` each compile as one crate
// root (`autotests = false` for the latter), so a file that the root does not
// reach is never compiled and its tests never run.

use std::collections::BTreeSet;
use std::path::Path;

/// The value of a one-line `#[path = "..."]` attribute. Only a double-quoted
/// string counts, as in Rust itself.
fn path_attr(attr: &str) -> Option<&str> {
    let inner = attr.trim().strip_prefix("#[")?.strip_suffix(']')?;
    let rest = inner.trim().strip_prefix("path")?.trim_start();
    let rest = rest.strip_prefix('=')?.trim();
    rest.strip_prefix('"')?.strip_suffix('"')
}

/// `[` minus `]` in `line`, ignoring brackets inside string and char literals
/// and anything after a `//` comment.
fn bracket_depth(line: &str) -> i32 {
    let chars: Vec<char> = line.chars().collect();
    let (mut depth, mut i, mut in_str) = (0, 0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            match c {
                '\\' => i += 1,
                '"' => in_str = false,
                _ => {}
            }
        } else {
            match c {
                '"' => in_str = true,
                '/' if chars.get(i + 1) == Some(&'/') => break,
                // A char literal: 'x' or '\x'. A lone ' (a lifetime) is skipped.
                '\'' if chars.get(i + 1) == Some(&'\\') && chars.get(i + 3) == Some(&'\'') => {
                    i += 3
                }
                '\'' if chars.get(i + 2) == Some(&'\'') => i += 2,
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    depth
}

/// Whether `line` starts a Rust item (or a macro invocation), which an
/// attribute can legitimately sit on.
fn starts_item(line: &str) -> bool {
    const KEYWORDS: [&str; 15] = [
        "fn", "pub", "use", "struct", "enum", "impl", "mod", "const", "static", "type", "trait",
        "extern", "unsafe", "async", "union",
    ];
    let word: String = line
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    KEYWORDS.contains(&word.as_str())
        || (!word.is_empty() && line[word.len()..].trim_start().starts_with('!'))
}

/// One `mod <name>;` declaration.
struct ModDecl {
    name: String,
    /// The `#[path]` value, if it has one.
    path: Option<String>,
    /// Whether a `#[cfg(..)]` or `#[cfg_attr(..)]` sits on it.
    cfg_gated: bool,
}

/// The `mod <name>;` declarations in `src`, each with the attributes directly
/// above it, plus the 1-based lines of any top-level `#[cfg]` that is followed
/// by neither another attribute nor an item. Such a line usually means an
/// attribute this reader split wrongly, so it is reported rather than
/// dropped. Comments between attributes are allowed, and an attribute that
/// spans several lines is read until its brackets balance.
fn mod_decls(src: &str) -> (Vec<ModDecl>, Vec<usize>) {
    let mut decls = Vec::new();
    let mut dangling = Vec::new();
    // Each attribute with the line it started on and whether it was top-level.
    let mut attrs: Vec<(String, usize, bool)> = Vec::new();
    let mut open: Option<(String, usize, bool, i32)> = None;
    let is_cfg = |a: &str| a.starts_with("#[cfg(") || a.starts_with("#[cfg_attr(");
    for (n, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if let Some((mut attr, start, top, depth)) = open.take() {
            attr.push(' ');
            attr.push_str(line);
            let depth = depth + bracket_depth(line);
            if depth > 0 {
                open = Some((attr, start, top, depth));
            } else {
                attrs.push((attr, start, top));
            }
            continue;
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with("#[") {
            let top = !raw.starts_with(char::is_whitespace);
            let depth = bracket_depth(line);
            if depth > 0 {
                open = Some((line.to_owned(), n + 1, top, depth));
            } else {
                attrs.push((line.to_owned(), n + 1, top));
            }
            continue;
        }
        let decl = line
            .strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line);
        if let Some(name) = decl.strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) {
            decls.push(ModDecl {
                name: name.trim().to_owned(),
                path: attrs
                    .iter()
                    .find_map(|(a, _, _)| path_attr(a))
                    .map(str::to_owned),
                cfg_gated: attrs.iter().any(|(a, _, _)| is_cfg(a)),
            });
        } else if !starts_item(line) {
            dangling.extend(
                attrs
                    .iter()
                    .filter(|(a, _, top)| *top && is_cfg(a))
                    .map(|(_, start, _)| *start),
            );
        }
        attrs.clear();
    }
    (decls, dangling)
}

/// The files that `decl`, declared in `file`, may load. A `#[path]` resolves
/// against `file`'s directory. A plain `mod x;` resolves to `x.rs` or
/// `x/mod.rs` in `file`'s directory when `file` owns that directory, and
/// under `<stem>/` otherwise. As in rustc, a file owns its directory if it is
/// a crate root, a `mod.rs`, or was itself loaded through a `#[path]`.
fn decl_candidates(file: &Path, owns_dir: bool, decl: &ModDecl) -> Vec<PathBuf> {
    let dir = file.parent().expect("module file has a parent directory");
    if let Some(path) = &decl.path {
        return vec![dir.join(path)];
    }
    let base = if owns_dir {
        dir.to_path_buf()
    } else {
        dir.join(file.file_stem().expect("module file has a stem"))
    };
    vec![
        base.join(format!("{}.rs", decl.name)),
        base.join(&decl.name).join("mod.rs"),
    ]
}

/// Every `.rs` file under `dir`, recursively. Panics on an unreadable entry so
/// an unreadable tree never passes.
fn rs_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("FAIL: cannot read {dir:?}: {e}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("FAIL: cannot list {dir:?}: {e}"))
            .path();
        if path.is_dir() {
            rs_files_under(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// What a crate root leaves unwired in its directory.
#[derive(Debug, Default, PartialEq)]
struct Unwired {
    /// `.rs` files anywhere under the directory, relative to it, that no
    /// module declaration reachable from `main.rs` (or from `also_owned`)
    /// loads. Sorted.
    files: Vec<String>,
    /// `<file>: mod <name>` for each `mod` behind a `#[cfg]` in a reached file
    /// whose target lies inside the directory, and `<file>:<line>` for each
    /// top-level `#[cfg]` followed by something that is not an item. Such a
    /// module may never compile in CI although it looks wired. Sorted.
    cfg_gated: Vec<String>,
}

/// Checks `dir` against its crate root `dir/main.rs`. `also_owned` names
/// top-level files that other packages' test targets compile; they are roots
/// too. Follows module declarations file by file, so a nested file is reached
/// only if some reached file declares it.
fn unwired(dir: &Path, also_owned: &[String]) -> Unwired {
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    let base = canon(dir).unwrap_or_else(|| panic!("FAIL: cannot resolve {dir:?}"));
    let rel = |p: &Path| p.strip_prefix(&base).unwrap_or(p).display().to_string();
    let mut reached = BTreeSet::new();
    let mut cfg_gated = Vec::new();
    // Crate roots own their directory.
    let mut todo: Vec<(PathBuf, bool)> = std::iter::once("main.rs".to_owned())
        .chain(also_owned.iter().cloned())
        .filter_map(|f| canon(&dir.join(f)))
        .map(|p| (p, true))
        .collect();
    if !todo.iter().any(|(p, _)| p.ends_with("main.rs")) {
        panic!("FAIL: cannot read {:?}", dir.join("main.rs"));
    }
    while let Some((file, owns_dir)) = todo.pop() {
        if !file.starts_with(&base) || !reached.insert(file.clone()) {
            continue;
        }
        let src = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("FAIL: cannot read {file:?}: {e}"));
        let (decls, dangling) = mod_decls(&src);
        cfg_gated.extend(dangling.iter().map(|n| format!("{}:{n}", rel(&file))));
        for decl in &decls {
            let targets: Vec<PathBuf> = decl_candidates(&file, owns_dir, decl)
                .iter()
                .filter_map(|p| canon(p))
                .collect();
            if decl.cfg_gated && targets.iter().any(|t| t.starts_with(&base)) {
                cfg_gated.push(format!("{}: mod {}", rel(&file), decl.name));
            }
            let child_owns_dir = decl.path.is_some();
            todo.extend(targets.into_iter().map(|t| {
                let is_mod_rs = t.file_name().is_some_and(|n| n == "mod.rs");
                (t, child_owns_dir || is_mod_rs)
            }));
        }
    }
    let mut all = Vec::new();
    rs_files_under(dir, &mut all);
    let mut files: Vec<String> = all
        .iter()
        .filter_map(|p| canon(p))
        .filter(|p| !reached.contains(p))
        .map(|p| rel(&p))
        .collect();
    files.sort();
    cfg_gated.sort();
    Unwired { files, cfg_gated }
}

fn assert_fully_wired(rel_dir: &str, also_owned: &[String]) {
    let found = unwired(&workspace_root().join(rel_dir), also_owned);
    assert!(
        found == Unwired::default(),
        "FAIL: {rel_dir} has tests that never compile or run:\n\
         - files no reachable module declaration loads: {:?}\n\
         - modules behind a #[cfg] (put any gate inside the module file \
         instead), or a #[cfg] this guard could not attach to an item: {:?}\n\
         Declare a top-level file in {rel_dir}/main.rs as\n\
         \n\
           #[path = \"<file>.rs\"]\n\
           mod <name>;\n\
         \n\
         and a nested one from the file that owns its directory.\n",
        found.files,
        found.cfg_gated
    );
}

/// Each workspace test target as `(package, target, src_path)`, from
/// `cargo metadata`. Reading Cargo's own view instead of parsing manifests
/// means comments, quoting and spacing cannot hide a target.
fn test_targets() -> Vec<(String, String, PathBuf)> {
    let out = std::process::Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .arg("--manifest-path")
        .arg(workspace_root().join("Cargo.toml"))
        .output()
        .expect("FAIL: cannot run cargo metadata");
    assert!(
        out.status.success(),
        "FAIL: cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("FAIL: cargo metadata printed invalid JSON");
    let mut targets = Vec::new();
    for package in meta["packages"]
        .as_array()
        .expect("metadata lists packages")
    {
        let pkg = package["name"].as_str().expect("package name");
        for target in package["targets"]
            .as_array()
            .expect("package lists targets")
        {
            let is_test = target["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|k| k == "test"));
            if is_test {
                targets.push((
                    pkg.to_owned(),
                    target["name"].as_str().expect("target name").to_owned(),
                    PathBuf::from(target["src_path"].as_str().expect("target src_path")),
                ));
            }
        }
    }
    targets
}

/// The names of `package`'s test targets, sorted.
fn test_target_names(package: &str) -> Vec<String> {
    let mut names: Vec<String> = test_targets()
        .into_iter()
        .filter(|(pkg, _, _)| pkg == package)
        .map(|(_, name, _)| name)
        .collect();
    names.sort();
    names
}

/// Every `tests/integration/` file must be compiled by some test target:
/// reached from `main.rs` (the `amplihack` package's `integration` binary), or
/// from a file that another package's test target compiles.
#[test]
fn every_integration_test_file_is_wired_into_a_test_binary() {
    let dir = workspace_root().join("tests/integration");
    let dir = std::fs::canonicalize(&dir).expect("tests/integration resolves");
    let owned_elsewhere: Vec<String> = test_targets()
        .into_iter()
        .filter(|(pkg, _, _)| pkg != "amplihack")
        .filter_map(|(_, _, src)| {
            let src = std::fs::canonicalize(src).ok()?;
            let rel = src.strip_prefix(&dir).ok()?;
            Some(rel.display().to_string())
        })
        .collect();
    assert_fully_wired("tests/integration", &owned_elsewhere);
}

/// Every `bins/amplihack/tests/` file must be reached from the `amplihack_bin`
/// root. With `autotests = false`, Cargo no longer builds these files on its
/// own.
#[test]
fn every_amplihack_bin_test_file_is_wired_into_a_test_binary() {
    assert_fully_wired("bins/amplihack/tests", &[]);
}

/// The `amplihack` package has exactly two test targets, by Cargo's own count.
/// If `autotests = false` were dropped, Cargo would list every
/// `bins/amplihack/tests/*.rs` file as another target here.
#[test]
fn amplihack_package_has_only_the_integration_and_amplihack_bin_targets() {
    assert_eq!(
        test_target_names("amplihack"),
        ["amplihack_bin", "integration"],
        "FAIL: add new bins/amplihack test files as modules of tests/main.rs, not as\n\
         new [[test]] entries, and keep `autotests = false`"
    );
}

/// The guard's helpers report each kind of gap they exist to catch.
#[test]
fn wiring_guard_reports_unwired_files_and_cfg_gated_modules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("tests");
    let outside = dir.path().join("outside.rs");
    std::fs::write(&outside, "").expect("write fixture");
    for sub in [
        "",
        "wired",
        "wired/deeper",
        "support",
        "stray",
        "chain",
        "chain/sub",
        "pathed",
    ] {
        std::fs::create_dir_all(root.join(sub)).expect("mkdir fixture");
    }
    for (file, body) in [
        (
            "main.rs",
            "#[path = \"wired.rs\"]\nmod wired;\nmod plain;\nmod support;\nmod chain;\n\
             #[path = \"pathed.rs\"]\nmod pathed;\n\
             #[cfg(feature = \"x\")]\n// comment\n#[path = \"gated.rs\"]\nmod gated;\n\
             #[cfg(any(\n    unix, // a ] in a comment\n    target_os = \"]\",\n))]\n\
             #[path = \"wrapped.rs\"]\nmod wrapped;\n\
             #[cfg(unix)]\nunix)]\n#[path = \"split.rs\"]\nmod split;\n",
        ),
        ("wired.rs", "#[path = \"wired/inner.rs\"]\nmod inner;\n"),
        // A #[path]-loaded file owns its directory, so its nested #[path] and a
        // cfg-gated mod there resolve beside it.
        (
            "wired/inner.rs",
            "#[path = \"deeper/leaf.rs\"]\nmod leaf;\n#[cfg(unix)]\nmod gated_inner;\n",
        ),
        ("wired/deeper/leaf.rs", ""),
        ("wired/gated_inner.rs", ""),
        ("wired/new_case.rs", ""),
        // A cfg-gated mod whose #[path] leaves the guarded directory is allowed.
        (
            "support/mod.rs",
            "mod helper;\n#[cfg(unix)]\n#[path = \"../../outside.rs\"]\npub mod outside;\n",
        ),
        ("support/helper.rs", ""),
        // A plain-mod chain: chain.rs is not mod.rs, so `mod sub;` is chain/sub.rs.
        ("chain.rs", "mod sub;\n"),
        ("chain/sub.rs", ""),
        // pathed.rs was loaded by #[path], so it owns tests/: `mod kid;` is
        // tests/kid.rs, and pathed/kid.rs is NOT reached.
        ("pathed.rs", "mod kid;\n"),
        ("kid.rs", ""),
        ("pathed/kid.rs", ""),
        ("plain.rs", ""),
        ("gated.rs", ""),
        ("wrapped.rs", ""),
        ("split.rs", ""),
        ("orphan.rs", ""),
        ("stray/lost.rs", ""),
    ] {
        std::fs::write(root.join(file), body).expect("write fixture");
    }
    assert_eq!(
        unwired(&root, &[]),
        Unwired {
            files: vec![
                "orphan.rs".to_owned(),
                "pathed/kid.rs".to_owned(),
                "stray/lost.rs".to_owned(),
                "wired/new_case.rs".to_owned(),
            ],
            cfg_gated: vec![
                "main.rs: mod gated".to_owned(),
                "main.rs: mod wrapped".to_owned(),
                "main.rs:18".to_owned(),
                "wired/inner.rs: mod gated_inner".to_owned(),
            ],
        }
    );
}

#[test]
fn bracket_depth_ignores_strings_chars_and_comments() {
    assert_eq!(bracket_depth("#[cfg(any("), 1);
    assert_eq!(bracket_depth("#[doc = \"]\"]"), 0);
    assert_eq!(bracket_depth("#[x(']', '\\'')]"), 0);
    assert_eq!(bracket_depth("#[cfg(any( // ]]"), 1);
    assert_eq!(path_attr("#[path = 'x.rs']"), None);
    assert_eq!(path_attr("#[path=\"x.rs\"]"), Some("x.rs"));
}
