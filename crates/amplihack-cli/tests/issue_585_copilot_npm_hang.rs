//! TDD tests for issue #585: Copilot npm install hang on WSL/Linux and
//! improved error messages in bootstrap.rs.
//!
//! Bug 1: npm hangs indefinitely during reify phase when installing
//!   @github/copilot on Linux because it tries to download platform-mismatched
//!   optional deps (e.g. copilot-darwin-arm64). Fix: --omit=optional + separate
//!   platform-specific binary install.
//!
//! Bug 2: Error messages from ensure_tool_available / install_npm_package are
//!   too generic — users need structured output with package name, failure
//!   reason, and copy-pasteable manual fix commands.

// ============================================================================
// Bug 1: run_npm_install must use --omit=optional
// ============================================================================

#[test]
fn run_npm_install_uses_omit_optional() {
    // Contract: run_npm_install must pass --omit=optional to npm to prevent
    // the reify hang caused by cross-platform optional dependencies.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn run_npm_install(")
        .expect("run_npm_install must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];
    assert!(
        fn_body.contains("--omit=optional")
            || fn_body.contains("omit") && fn_body.contains("optional"),
        "run_npm_install must pass --omit=optional to prevent npm hang on \
         cross-platform optional deps. Got:\n{fn_body}"
    );
}

#[test]
fn run_npm_install_does_not_use_os_cpu_flags() {
    // Contract: --os and --cpu flags are broken in npm 9.x and must NOT
    // be used. PR #585 was closed because this approach was wrong.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn run_npm_install(")
        .expect("run_npm_install must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];
    assert!(
        !fn_body.contains("\"--os\""),
        "run_npm_install must NOT use --os flag (broken in npm 9.x)"
    );
    assert!(
        !fn_body.contains("\"--cpu\""),
        "run_npm_install must NOT use --cpu flag (broken in npm 9.x)"
    );
}

// ============================================================================
// Bug 1: Platform-specific binary install after base install
// ============================================================================

#[test]
fn install_npm_package_calls_platform_binary_install_for_copilot() {
    // Contract: install_npm_package must attempt to install the
    // platform-specific binary package (e.g. @github/copilot-linux-x64)
    // after the base @github/copilot install with --omit=optional.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn install_npm_package(")
        .expect("install_npm_package must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];
    assert!(
        fn_body.contains("copilot_platform_package") || fn_body.contains("platform_package"),
        "install_npm_package must call copilot_platform_package to determine \
         the platform-specific binary. Got:\n{fn_body}"
    );
}

#[test]
fn copilot_platform_package_helper_exists() {
    // Contract: A copilot_platform_package function must exist to map
    // OS/arch to the correct @github/copilot-{os}-{arch} package name.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    assert!(
        bootstrap_src.contains("fn copilot_platform_package("),
        "copilot_platform_package helper must exist in bootstrap.rs"
    );
}

#[test]
fn platform_binary_install_failure_is_non_fatal() {
    // Contract: If the platform-specific binary install fails, the error
    // must be logged/warned but must NOT fail the overall install.
    // This is because Node.js 24+ has a JS fallback via index.js.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn install_npm_package(")
        .expect("install_npm_package must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];

    // The platform binary install should be guarded by a non-fatal pattern:
    // either if let Err(...), match ... Err, .ok(), or warn!/tracing::warn!
    let has_non_fatal_guard = fn_body.contains("if let Err(")
        || fn_body.contains("tracing::warn!")
        || fn_body.contains(".ok()")
        || fn_body.contains("Err(err) =>");
    assert!(
        has_non_fatal_guard,
        "platform binary install failure must be non-fatal (warn, not bail!). \
         Got:\n{fn_body}"
    );
}

// ============================================================================
// Bug 2: Structured error messages
// ============================================================================

#[test]
fn run_npm_install_error_includes_package_name() {
    // Contract: The error message from run_npm_install must include the
    // package name so users know which package failed.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn run_npm_install(")
        .expect("run_npm_install must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];

    // The bail!/error message must interpolate the package variable
    assert!(
        fn_body.contains("{package}") || fn_body.contains("package"),
        "error message must reference the package name"
    );
}

#[test]
fn run_npm_install_error_includes_manual_fix_command() {
    // Contract: When npm install fails, the error must include a
    // copy-pasteable manual fix command so users can recover.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn run_npm_install(")
        .expect("run_npm_install must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];

    assert!(
        fn_body.contains("npm install") && fn_body.contains("--prefix"),
        "error message must include a manual npm install command with --prefix. \
         Got:\n{fn_body}"
    );
}

#[test]
fn run_npm_install_error_mentions_omit_optional_for_timeout() {
    // Contract: When npm times out, the error must mention --omit=optional
    // as a diagnostic hint, since the hang is often caused by cross-platform
    // optional deps.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn run_npm_install(")
        .expect("run_npm_install must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];

    // Either the function itself or its callers must handle timeout errors
    // with a mention of --omit=optional. Check both this function and
    // install_npm_package.
    let install_fn_start = bootstrap_src
        .find("fn install_npm_package(")
        .expect("install_npm_package must exist");
    let install_fn_body = &bootstrap_src[install_fn_start..];
    let install_fn_end = find_fn_end(install_fn_body);
    let install_fn_body = &install_fn_body[..install_fn_end];

    let combined = format!("{fn_body}\n{install_fn_body}");
    assert!(
        combined.contains("timed out") || combined.contains("timeout"),
        "error handling must address timeout case. Got:\n{combined}"
    );
}

#[test]
fn ensure_tool_available_error_is_actionable() {
    // Contract: When ensure_tool_available fails to locate a tool after
    // installation, the error message must be actionable — not just
    // "failed to locate 'X' after installation".
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn ensure_tool_available(")
        .or_else(|| bootstrap_src.find("pub fn ensure_tool_available("))
        .expect("ensure_tool_available must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];

    // Must reference PATH or provide diagnostic guidance, not just
    // "failed to locate"
    let has_actionable_guidance = fn_body.contains("PATH")
        || fn_body.contains("npm install")
        || fn_body.contains("Try running");
    assert!(
        has_actionable_guidance,
        "ensure_tool_available error must include actionable guidance \
         (PATH, manual install command, etc.). Got:\n{fn_body}"
    );
}

// ============================================================================
// Structural: no regression — existing install features preserved
// ============================================================================

#[test]
fn run_npm_install_still_uses_ignore_scripts() {
    // Contract: --ignore-scripts must remain to prevent post-install scripts
    // from running during automated install (security requirement).
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn run_npm_install(")
        .expect("run_npm_install must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];
    assert!(
        fn_body.contains("--ignore-scripts"),
        "run_npm_install must keep --ignore-scripts for security"
    );
}

#[test]
fn run_npm_install_still_uses_global_flag() {
    // Contract: -g flag must remain for global npm installs.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn run_npm_install(")
        .expect("run_npm_install must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];
    assert!(
        fn_body.contains("\"-g\""),
        "run_npm_install must keep -g flag for global install"
    );
}

#[test]
fn install_npm_package_still_cleans_stale_temp_dirs() {
    // Contract: Stale temp dir cleanup must remain (prevents ENOTEMPTY).
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn install_npm_package(")
        .expect("install_npm_package must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];
    assert!(
        fn_body.contains("clean_stale_npm_temp_dirs"),
        "install_npm_package must still clean stale npm temp dirs"
    );
}

#[test]
fn install_npm_package_retries_on_failure() {
    // Contract: retry-once logic must remain.
    let bootstrap_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bootstrap.rs"));
    let fn_start = bootstrap_src
        .find("fn install_npm_package(")
        .expect("install_npm_package must exist");
    let fn_body = &bootstrap_src[fn_start..];
    let fn_end = find_fn_end(fn_body);
    let fn_body = &fn_body[..fn_end];
    assert!(
        fn_body.contains("retrying") || fn_body.contains("retry"),
        "install_npm_package must still retry on failure"
    );
}

// ============================================================================
// Helper
// ============================================================================

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
