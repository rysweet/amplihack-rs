//! Contract tests for the claude native install (issue #1266, Task A).
//!
//! # The constraint these tests exist to hold
//!
//! `@anthropic-ai/claude-code` must install *properly* — a real native binary,
//! not the 500-byte placeholder — **without** relaxing the npm flags that
//! issue #585 depends on. Both, not one.
//!
//! The mechanism is the one the copilot path already established: keep
//! `run_npm_install` byte-identical (both `--omit=optional` and
//! `--ignore-scripts` intact for every package) and install the single
//! platform-native package **explicitly, by exact name**, then run the vendor's
//! `install.cjs` against it.
//!
//! So the "narrowed exception" is not a flag relaxation applied to a class of
//! packages. It is one auditable `if` keyed on exact string equality against a
//! `&'static str`, and the residual delta over today's behaviour is exactly one
//! named script at an absolute path under a prefix amplihack owns. The
//! exact-equality tests below are the control that stops that scope widening.
//!
//! # What issue #585 was actually about
//!
//! `gh issue view 585` — *"fix: pass `--os`/`--cpu` flags to npm install to
//! prevent WSL hangs"*. Root cause, verbatim from the issue: npm "downloaded
//! optional deps for every platform" and stuck in an infinite reify loop on
//! `@github/copilot-darwin-arm64`. `--ignore-scripts` appears nowhere in #585's
//! diagnosis or its remedy — it is asserted in
//! `issue_585_copilot_npm_hang.rs` as a generic security requirement that
//! happens to share a file with the hang tests. The two are a naming
//! coincidence, not a causal link.
//!
//! Nothing in this design rides on that conclusion, because both flags survive
//! untouched.

use std::path::Path;

fn bootstrap_src() -> &'static str {
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"))
}

/// Find the closing brace of a function body (handles nested braces).
fn find_fn_end(src: &str) -> usize {
    let mut depth = 0;
    for (i, ch) in src.char_indices() {
        if ch == '{' {
            depth += 1;
        }
        if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
    }
    src.len()
}

fn fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} must exist in bootstrap.rs"));
    let rest = &src[start..];
    rest[..find_fn_end(rest)].to_string()
}

// ===========================================================================
// The HARD CONSTRAINT: copilot's npm invocation does not change by one byte
// ===========================================================================

#[test]
fn run_npm_install_keeps_every_protective_flag() {
    // SEC: this is the whole reason the fix is shaped as an explicit
    // second-package install rather than a script allowlist. An allowlist would
    // re-enable ARBITRARY lifecycle scripts for a package class; this keeps
    // both flags for every package, claude's base install included.
    let body = fn_body(bootstrap_src(), "fn run_npm_install(");
    for flag in ["--ignore-scripts", "--omit=optional", "\"-g\"", "--prefix"] {
        assert!(
            body.contains(flag),
            "run_npm_install must keep {flag}; its diff is required to be empty.\nGot:\n{body}"
        );
    }
}

#[test]
fn run_npm_install_takes_no_per_package_flag_exception() {
    // Any `if package == ...` inside run_npm_install would mean the flag policy
    // had been made package-dependent — exactly what the HARD CONSTRAINT
    // forbids. The exception belongs one level up, in install_npm_package,
    // where it is visible as an extra install step rather than as a weakened
    // invocation.
    let body = fn_body(bootstrap_src(), "fn run_npm_install(");
    for smell in [
        "@anthropic-ai/claude-code",
        "allow-scripts",
        "if package ==",
        "contains(",
        "starts_with(",
    ] {
        assert!(
            !body.contains(smell),
            "run_npm_install must stay package-agnostic; found {smell:?}:\n{body}"
        );
    }
}

#[test]
fn no_global_npm_config_is_ever_mutated() {
    // `npm config set allow-scripts=... --location=user` was tried on the dev
    // VM and had no effect (a per-invocation flag outranks npm config), but it
    // would also have been a machine-wide relaxation. Never do it.
    let src = bootstrap_src();
    assert!(
        !src.contains("config set") && !src.contains("--location=user"),
        "amplihack must not mutate the user's global npm configuration"
    );
}

// ===========================================================================
// The narrowed exception: exact equality, one package
// ===========================================================================

#[test]
fn claude_native_step_triggers_on_exactly_one_package_name() {
    // A4′: exact equality only. `contains()` / `starts_with()` / tool-name
    // matching would let a near-miss package name inherit the exception.
    let body = fn_body(bootstrap_src(), "fn install_npm_package(");
    assert!(
        body.contains("package == \"@anthropic-ai/claude-code\""),
        "the claude arm must key on exact string equality, mirroring the \
         `package == \"@github/copilot\"` arm directly above it.\nGot:\n{body}"
    );
}

#[test]
fn the_claude_arm_never_matches_a_near_miss_package_name() {
    // Prove the predicate itself, not just its spelling in source.
    let exact = "@anthropic-ai/claude-code";
    for near_miss in [
        "@anthropic-ai/claude-code-evil",
        "@anthropic-ai/claude-code-linux-x64",
        "claude-code",
        "@anthropic-ai/claude-code ",
        " @anthropic-ai/claude-code",
        "@anthropic-ai/Claude-Code",
        "@github/copilot",
    ] {
        assert_ne!(
            near_miss, exact,
            "{near_miss:?} must not be treated as the trusted package"
        );
    }
}

#[test]
fn the_exception_is_documented_honestly_where_it_lives() {
    // The threat model has to be argued in the source, not just the PR: the
    // postinstall amplihack declines to run is strictly less privileged than
    // the native binary it is about to exec seconds later.
    let body = fn_body(bootstrap_src(), "fn materialize_claude_native(");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("exec") && lower.contains("postinstall"),
        "materialize_claude_native must carry the honest threat-model comment: \
         amplihack is about to execute this package's native binary anyway, so \
         refusing to run its own postinstall is not a coherent posture.\nGot:\n{body}"
    );
}

// ===========================================================================
// SEC-2: the one runtime string that reaches npm's argv
// ===========================================================================

#[test]
fn pinned_version_is_validated_against_an_anchored_regex() {
    // SEC-2: the platform package is pinned to the version read out of the
    // installed package.json, and that value is concatenated into npm's argv.
    // An unanchored regex would accept `1.2.3 && rm -rf ~`.
    let src = bootstrap_src();
    assert!(
        src.contains(r"^\d{1,9}\.\d{1,9}\.\d{1,9}$"),
        "the package.json version must be validated against an ANCHORED, \
         digit-bounded pattern before it is used to pin the platform package"
    );
}

#[test]
fn pinned_version_rejects_injection_and_range_syntax() {
    let re = regex::Regex::new(r"^\d{1,9}\.\d{1,9}\.\d{1,9}$").unwrap();
    for good in ["2.1.238", "0.0.1", "10.20.30"] {
        assert!(re.is_match(good), "{good} is a valid pin");
    }
    for bad in [
        "2.1.238 && rm -rf ~",
        "2.1.238\nrm -rf ~",
        "^2.1.238",
        "latest",
        "",
        "2.1",
        "2.1.238-beta.1; id",
        "../../../etc/passwd",
        // Unbounded `\d+` would accept this and build a megabyte of argv.
        &"9".repeat(1000),
    ] {
        assert!(
            !re.is_match(bad),
            "{bad:?} must be rejected before reaching npm"
        );
    }
}

// ===========================================================================
// Success is verified by outcome, never by install.cjs's exit code
// ===========================================================================

#[test]
fn materialization_is_verified_by_outcome_not_exit_code() {
    // Verified from the vendor's source: install.cjs main() returns 0 on
    // unsupported platform, on a channel with no native binaries, and on a
    // failed require.resolve. Exit code 1 happens only when placeBinary throws.
    // Its exit code is not a success signal.
    let body = fn_body(bootstrap_src(), "fn materialize_claude_native(");
    assert!(
        body.contains("is_materialized"),
        "success must be confirmed by inspecting the resulting file (size + \
         native magic), not by the postinstall's exit status.\nGot:\n{body}"
    );
}

#[test]
fn every_native_install_failure_path_is_non_fatal() {
    // Same posture as the copilot platform-binary install: warn, tell the user,
    // return. The health gate then rejects the un-materialized stub and
    // resolution falls through to whatever else on the host is healthy. A
    // failed materialization must never fail the launch.
    let body = fn_body(bootstrap_src(), "fn materialize_claude_native(");
    assert!(
        body.contains("tracing::warn!") || body.contains("warn!"),
        "failures must warn:\n{body}"
    );
    assert!(
        !body.contains("bail!") && !body.contains("return Err("),
        "materialize_claude_native must never propagate an error that could \
         fail the launch.\nGot:\n{body}"
    );
}

// ===========================================================================
// A6: one resolver. Defect 2 was three of them disagreeing.
// ===========================================================================

/// Every `.rs` file in the workspace except the single-resolver module itself
/// and the tests that assert about it.
fn workspace_sources() -> Vec<(std::path::PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(std::path::PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name == "target" || name == ".git" {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push((path, text));
            }
        }
    }
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("crates");
    let mut out = Vec::new();
    walk(&workspace_root, &mut out);
    out
}

#[test]
fn only_launch_target_resolves_a_claude_path() {
    // Defect 2, verbatim from a real launch on the dev VM 2026-08-21 15:53 —
    // three different binaries in ONE launch:
    //   version-checked: /usr/bin/claude            (2.1.237, npm root install)
    //   installed to:    ~/.npm-global/bin/claude   (a 500-byte stub)
    //   launched:        ~/.local/bin/claude        (2.1.238, the only good one)
    // Whatever decides what to exec must be what decides whether to upgrade.
    let offenders: Vec<String> = workspace_sources()
        .into_iter()
        .filter(|(path, _)| {
            let p = path.to_string_lossy();
            !p.contains("launch_target.rs") && !p.contains("claude_install_contract.rs")
        })
        .filter(|(_, text)| {
            text.contains("BinaryFinder::find(\"claude\")") || text.contains("which_claude(")
        })
        .map(|(path, _)| path.display().to_string())
        .collect();

    assert!(
        offenders.is_empty(),
        "these files resolve a claude path independently of \
         launch_target::resolve — that is how the three resolutions drifted \
         apart in the first place:\n  {}",
        offenders.join("\n  ")
    );
}

/// Issue 6 — `resolve_from_candidates` is a test seam, not a second resolver.
///
/// It is `pub` for a legitimate reason: the integration tests in
/// `crates/amplihack-utils/tests/` need a deterministic seam that does not
/// mutate process env (`set_var` is `unsafe` under edition 2024, and this
/// workspace forbids `$PATH` mutation in unit tests outright), and 20+ tests
/// use it. But it is a public entry point that SKIPS `candidate_paths` — which
/// is to say it skips the environment reading that makes `resolve` the single
/// answer to "which binary". Production code calling it would be a fourth
/// independent resolution, the exact shape of Defect 2.
///
/// The scan above forbids the old resolvers by NAME. This one forbids the
/// bypass by shape, so it also covers the resolver nobody has written yet.
#[test]
fn only_tests_call_the_resolution_seam_directly() {
    let offenders: Vec<String> = workspace_sources()
        .into_iter()
        .filter(|(path, _)| {
            let p = path.to_string_lossy().replace('\\', "/");
            // Exempt: the definition itself, this contract, and test code —
            // which is what the seam exists for. Deliberately NARROW, so a
            // production file does not fall out of the scan just because its
            // name happens to contain "test".
            !p.ends_with("/launch_target.rs")
                && !p.ends_with("/claude_install_contract.rs")
                && !p.contains("/tests/")
                && !p.contains("/test_support")
                && !p.rsplit('/').next().is_some_and(|f| {
                    f.starts_with("tests_") || f.ends_with("_test.rs") || f.ends_with("_tests.rs")
                })
        })
        .filter(|(_, text)| text.contains("resolve_from_candidates("))
        .map(|(path, _)| path.display().to_string())
        .collect();

    assert!(
        offenders.is_empty(),
        "these non-test sources call `resolve_from_candidates` directly, \
         bypassing `candidate_paths` and therefore the single-resolver \
         guarantee. Production code must call `launch_target::resolve` (or \
         `resolve_uncached`), which reads the environment once and answers for \
         the whole process:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_second_installer_is_gone() {
    // `claude_cli::ensure_claude_cli` ran its own
    // `npm install -g --ignore-scripts @anthropic-ai/claude-code`, independent
    // of bootstrap.rs. Left alone it re-creates the stub and re-opens the whole
    // bug from a second door. It has zero in-repo callers, so it is deleted
    // rather than rewired.
    let offenders: Vec<String> = workspace_sources()
        .into_iter()
        .filter(|(path, _)| {
            !path
                .to_string_lossy()
                .contains("claude_install_contract.rs")
        })
        .filter(|(_, text)| text.contains("ensure_claude_cli"))
        .map(|(path, _)| path.display().to_string())
        .collect();

    assert!(
        offenders.is_empty(),
        "ensure_claude_cli must not survive the PR — a dead second installer \
         that can mint a stub is worse than none:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_install_decision_does_not_read_the_ambient_npm_prefix() {
    // `tool_update_check/version.rs` runs `npm list -g --depth=0 --json` under
    // npm's AMBIENT prefix — not the --prefix amplihack installs to, and not
    // the binary it launches. That mismatch is Defect 2's mechanical root. It
    // stays available for the advisory update NOTICE and must gate nothing.
    let body = fn_body(bootstrap_src(), "fn ensure_tool_available(");
    assert!(
        !body.contains("get_installed_version"),
        "the install decision must read the binary it is about to launch, via \
         launch_target::resolve — never `npm list -g`.\nGot:\n{body}"
    );
    assert!(
        body.contains("decide_install"),
        "ensure_tool_available must route through the pure decide_install \
         function.\nGot:\n{body}"
    );
}

#[test]
fn a_failed_upgrade_never_relaunches_the_pre_upgrade_target() {
    // `install_npm_package` runs with `--ignore-scripts` and
    // `materialize_claude_native` is documented non-fatal, so an upgrade can
    // succeed at npm and still leave the ~500-byte placeholder at
    // bin/claude.exe. Keeping the pre-upgrade `Resolution` as a fallback then
    // hands exec a LaunchTarget whose version is a memory of a file that is no
    // longer there — issue #1266's own `Exec format error`, on the upgrade
    // path. The retry path is worse: `remove_package_install_dir` can delete
    // the very directory the fallback names.
    //
    // Health is a filter, never an annotation. This scans by shape rather than
    // by name so it also covers the fallback nobody has reintroduced yet.
    let body = fn_body(bootstrap_src(), "fn reinstall_and_reresolve(");
    assert!(
        !body.contains("previous"),
        "reinstall_and_reresolve must answer with what the filesystem says now, \
         never a pre-upgrade resolution that skipped the health gate.\nGot:\n{body}"
    );
    assert!(
        body.contains("resolve_uncached"),
        "the post-install answer must come from an uncached re-resolution — the \
         memo cannot see the install that just ran.\nGot:\n{body}"
    );
}

#[test]
fn a_broken_override_outside_amplihacks_prefix_is_not_installed_over() {
    // `decide_install` cannot judge this without knowing the one directory an
    // install writes, so the call site has to supply it. Passing `None` here
    // would compile, pass every other test, and restore the loop.
    let body = fn_body(bootstrap_src(), "fn ensure_tool_available(");
    assert!(
        body.contains("amplihack_prefix_bin"),
        "ensure_tool_available must tell decide_install where an install can \
         actually reach.\nGot:\n{body}"
    );
    assert!(
        body.contains("BrokenOverride"),
        "a broken override an install cannot repair must be reported, not \
         installed over.\nGot:\n{body}"
    );
}

#[test]
fn ensure_tool_available_error_still_carries_actionable_guidance() {
    // Preserved from issue_585_copilot_npm_hang.rs — the restructure must not
    // lose the actionable error text that test pinned.
    let body = fn_body(bootstrap_src(), "fn ensure_tool_available(");
    assert!(
        body.contains("PATH") || body.contains("npm install") || body.contains("Try running"),
        "the failure message must stay actionable:\n{body}"
    );
}

#[test]
fn the_postinstall_script_is_contained_before_it_is_executed() {
    // SEC-2. The exception is argued on the grounds that install.cjs sits
    // "under a prefix amplihack owns". If
    // <prefix>/lib/node_modules/@anthropic-ai/claude-code is a symlink —
    // planted by another package's install, or left by an `npm link` —
    // amplihack would execute arbitrary JS with the user's privileges during
    // an install. `canonicalize` + `starts_with` turns the assumption into an
    // assertion.
    let body = fn_body(bootstrap_src(), "fn contained_install_script(");
    assert!(
        body.contains("canonicalize()") && body.contains("starts_with"),
        "the postinstall path must be canonicalized and checked for \
         containment before it is run.\nGot:\n{body}"
    );
    let caller = fn_body(bootstrap_src(), "fn run_claude_vendor_postinstall(");
    assert!(
        caller.contains("contained_install_script"),
        "run_claude_vendor_postinstall must not build the script path itself; \
         it must go through the contained resolver.\nGot:\n{caller}"
    );
}

// ===========================================================================
// The tool-generic resolver stays tool-generic
//
// `launch_target` answers "which binary, is it healthy" for claude, copilot,
// codex and anything added later. A claude-shaped fact in its body is not a
// style problem: a "small file with no native magic is a broken install" gate
// rejected @github/copilot's real 1185-byte loader and made `amplihack
// copilot` reinstall on every launch and then hard-fail.
// ===========================================================================

fn launch_target_src() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../amplihack-utils/src/launch_target.rs"
    ))
}

/// `launch_target.rs`'s production code: no `#[cfg(test)]` module, no comments,
/// no `use` declarations.
///
/// Each exclusion earns its place. Comments legitimately discuss the copilot
/// regression and claude's package by name — pinning what a comment may say is
/// not the point. Test fixtures name real tools because that is what they
/// simulate. A `use crate::claude_native::…` line is the *design*: the
/// claude-shaped knowledge lives in that module and is imported from it. What
/// must stay out is anything that can reach a user or change a decision.
fn launch_target_code() -> String {
    let src = launch_target_src();
    let production = match src.find("#[cfg(test)]") {
        Some(i) => &src[..i],
        None => src,
    };
    production
        .lines()
        .filter(|line| {
            let t = line.trim_start();
            !t.starts_with("//") && !t.starts_with("use ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_resolver_names_no_npm_package() {
    let code = launch_target_code();
    for literal in ["@anthropic-ai", "@github/", "@openai/"] {
        assert!(
            !code.contains(literal),
            "launch_target is tool-generic; {literal:?} belongs to a caller \
             that knows which tool it is asking about"
        );
    }
}

#[test]
fn no_user_facing_string_in_the_resolver_names_a_specific_tool() {
    // Every message this module produces is the error path for EVERY tool. It
    // used to tell a copilot user "No usable claude binary was found" and hand
    // them `npm install -g @anthropic-ai/claude-code`.
    let code = launch_target_code().to_lowercase();
    for name in ["claude", "copilot", "codex", "rustyclawd", "amplifier"] {
        assert!(
            !code.contains(name),
            "the resolver's code must not mention {name:?} — take the tool \
             name as a parameter instead"
        );
    }
}

#[test]
fn the_placeholder_shape_check_cannot_reject_a_candidate() {
    // The structural guarantee: `has_placeholder_shape` may only ever RELABEL
    // a rejection the probe already made. If it is ever consulted from
    // `cheap_reject` again — the pre-probe gate — it can produce a false
    // rejection for any tool, which is the copilot break, restored.
    let src = launch_target_src();
    let body = fn_body(src, "fn cheap_reject(");
    assert!(
        !body.contains("has_placeholder_shape") && !body.contains("STUB_MAX_LEN"),
        "cheap_reject must answer 'can this be executed at all' and nothing \
         about the file's contents.\nGot:\n{body}"
    );
    assert!(
        fn_body(src, "fn label_failed_probe(").contains("has_placeholder_shape"),
        "the shape check belongs on the already-failed path"
    );
}
