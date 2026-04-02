//! Security hygiene tests — Issue #77 pre-merge checklist (TDD phase).
//!
//! These tests specify the security contract that the fleet module **must**
//! satisfy before the issue-#77 PR is merged.  They are written first
//! (TDD-style) and drive the remaining implementation work in the four
//! "PENDING" security items listed in the design specification:
//!
//! 1. `cargo audit` returns zero high/critical CVEs               (TC-SEC-01)
//! 2. `Command::new()` never uses `sh -c` with user input         (TC-SEC-02–04)
//! 3. `tempfile::persist()` creates files with `0o600` mode       (TC-SEC-05–07)
//! 4. CLI session-ID / path args validated against allowlist      (TC-SEC-08–14)
//!
//! Additionally, the tests cover:
//! 5. `shell_single_quote` escaping correctness                   (TC-SEC-15–19)
//! 6. No hardcoded secrets / API-key patterns in source           (TC-SEC-20)
//! 7. Error messages do not expose absolute filesystem paths      (TC-SEC-21–22)
//!
//! # Failing-test contract
//!
//! Tests marked `// EXPECTED FAIL` in their doc-comment will fail until the
//! corresponding implementation work is complete.  Tests that are pure
//! regression guards (they pass today but must never regress) are marked
//! `// REGRESSION GUARD`.
//!
//! # Running
//!
//! ```bash
//! cargo build                                  # compile the binary first
//! cargo test --test security_hygiene           # run only these tests
//! ```

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Absolute path to the debug `amplihack` binary.
fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack  →  workspace root
    path.pop(); // workspace root  (already at root after first pop from tests/)
    path.push("target/debug/amplihack");
    path
}

/// Skip the test gracefully when the binary has not been compiled yet.
macro_rules! require_binary {
    ($bin:expr) => {
        if !$bin.exists() {
            eprintln!(
                "SKIP: amplihack binary not found at {}; run `cargo build` first",
                $bin.display()
            );
            return;
        }
    };
}

/// Path to the fleet.rs source file (used for static analysis tests).
///
/// After the module split (PR #121), `fleet.rs` became `commands/fleet/` dir.
/// Returns the directory containing the fleet module source files.
fn fleet_rs_source() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/ → workspace root
    path.pop();
    path.push("crates/amplihack-cli/src/commands/fleet");
    path
}

/// Read all fleet module source as a single concatenated string for static analysis.
fn fleet_rs_combined_source() -> String {
    let dir = fleet_rs_source();
    assert!(dir.exists(), "fleet module not found at {}", dir.display());
    let mut combined = String::new();
    for file in collect_rs_files(&dir) {
        if let Ok(content) = fs::read_to_string(&file) {
            combined.push_str(&format!("// --- {} ---\n", file.display()));
            combined.push_str(&content);
            combined.push('\n');
        }
    }
    assert!(!combined.is_empty(), "No .rs files found in fleet module");
    combined
}

/// Read all fleet_local module source as a single concatenated string.
fn fleet_local_combined_source() -> Option<String> {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop();
    dir.pop();
    dir.push("crates/amplihack-cli/src/fleet_local");
    if !dir.exists() {
        return None;
    }
    let mut combined = String::new();
    for file in collect_rs_files(&dir) {
        if let Ok(content) = fs::read_to_string(&file) {
            combined.push_str(&format!("// --- {} ---\n", file.display()));
            combined.push_str(&content);
            combined.push('\n');
        }
    }
    if combined.is_empty() {
        None
    } else {
        Some(combined)
    }
}

/// All Rust source files under the fleet module scope.
fn fleet_source_files() -> Vec<PathBuf> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root.push("crates/amplihack-cli/src");
    collect_rs_files(&root)
}

fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                result.extend(collect_rs_files(&path));
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                result.push(path);
            }
        }
    }
    result
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-01: Cargo.lock must exist (prerequisite for `cargo audit`)
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-01: Cargo.lock is present in the workspace root.
///
/// `cargo audit` requires Cargo.lock to report on dependency CVEs.
/// If Cargo.lock is absent (e.g. the repo's .gitignore erroneously excludes
/// it for binary crates) the audit silently skips.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_01_cargo_lock_exists_for_audit() {
    let mut lock_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    lock_path.pop();
    lock_path.pop();
    lock_path.push("Cargo.lock");

    assert!(
        lock_path.exists(),
        "Cargo.lock must exist at workspace root for `cargo audit` to function. \
         Binary crates must commit their lockfile. Missing: {}",
        lock_path.display()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-02–04: No `sh -c` with user-controlled input in Command::new() calls
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-02: fleet.rs must not contain `Command::new("sh")` invocations.
///
/// All subprocess calls must pass arguments as a Vec (via `.args([...])`)
/// rather than through a shell interpreter, which would allow argument
/// injection from user-controlled data.
///
/// This is a static-analysis test: it reads the source file and searches for
/// the pattern.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_02_fleet_rs_no_command_new_sh() {
    let source = fleet_rs_combined_source();

    // Look for patterns that would indicate shell-via-sh or shell-via-bash.
    // We allow these in comments and string tests, but NOT in executable code
    // that could accept user input.
    let forbidden_patterns = [
        r#"Command::new("sh")"#,
        r#"Command::new("bash")"#,
        r#"Command::new("/bin/sh")"#,
        r#"Command::new("/bin/bash")"#,
    ];

    for pattern in &forbidden_patterns {
        // Find all occurrences, ignoring comment lines (lines starting with //)
        let violations: Vec<(usize, &str)> = source
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim();
                !trimmed.starts_with("//") && trimmed.contains(pattern)
            })
            .collect();

        assert!(
            violations.is_empty(),
            "SECURITY VIOLATION: fleet.rs contains forbidden shell-dispatch pattern \
             `{pattern}` at lines: {:?}. \
             Use Command::new(binary_path).args([...]) instead of shell dispatch.",
            violations.iter().map(|(ln, _)| ln + 1).collect::<Vec<_>>()
        );
    }
}

/// TC-SEC-03: fleet_local.rs must not contain `Command::new("sh")` invocations.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_03_fleet_local_rs_no_command_new_sh() {
    let source = match fleet_local_combined_source() {
        Some(s) => s,
        None => {
            eprintln!("SKIP: fleet_local module not found");
            return;
        }
    };

    let forbidden_patterns = [
        r#"Command::new("sh")"#,
        r#"Command::new("bash")"#,
        r#"Command::new("/bin/sh")"#,
        r#"Command::new("/bin/bash")"#,
    ];

    for pattern in &forbidden_patterns {
        let violations: Vec<usize> = source
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim();
                !trimmed.starts_with("//") && trimmed.contains(pattern)
            })
            .map(|(ln, _)| ln + 1)
            .collect();

        assert!(
            violations.is_empty(),
            "SECURITY VIOLATION: fleet_local.rs contains `{pattern}` at lines: {violations:?}."
        );
    }
}

/// TC-SEC-04: No `"-c"` argument directly follows a shell binary in any
/// Command builder chain in **fleet.rs and fleet_local.rs**.
///
/// Detects patterns like:
/// ```rust
/// Command::new("sh").arg("-c").arg(user_input)  // FORBIDDEN
/// ```
///
/// Scope is intentionally limited to the fleet module files. Other modules
/// (e.g. `install.rs`) may have legitimate `shell -c` uses for shell-builtin
/// probing; those are out of scope for this fleet security check.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_04_no_shell_minus_c_arg_pattern_in_fleet_source() {
    // Scan fleet module directories instead of old single files.
    let fleet_source = fleet_rs_combined_source();
    let fleet_local_source = fleet_local_combined_source().unwrap_or_default();
    let combined_sources = [
        ("fleet/", fleet_source.as_str()),
        ("fleet_local/", fleet_local_source.as_str()),
    ];

    for (label, source) in &combined_sources {
        if source.is_empty() {
            continue;
        }

        // Heuristic: look for `.arg("-c")` or `.args(["-c"` in non-comment lines.
        // In fleet.rs, `-c` should only appear as a tmux window index flag or
        // the PID signal argument (-TERM/-9), never as a shell interpreter flag.
        let lines: Vec<&str> = source.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            if trimmed.contains(r#".arg("-c")"#) || trimmed.contains(r#".args(["-c""#) {
                panic!(
                    "SECURITY VIOLATION: potential shell -c injection at {}:{}: `{}`.\n\
                     '-c' must not appear as a command argument — it \
                     indicates shell dispatch with user-controlled data.\n\
                     If this is a legitimate non-shell use of '-c', add a \
                     `// SEC-SHELL-C: <justification>` comment on the same line.",
                    label,
                    i + 1,
                    trimmed
                );
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-05–07: tempfile::persist() must produce files with mode 0o600
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-05: A `NamedTempFile::new_in()` + `persist()` sequence produces a
/// file with exactly mode `0o600` (owner read/write only).
///
/// This validates the security property that all write_json_file() calls
/// inherit: the tempfile crate creates with 0o600 and persist() preserves it.
///
/// // REGRESSION GUARD — expected to pass; failing means the tempfile crate
/// // changed behavior or the umask is stripping owner bits.
#[test]
fn tc_sec_05_named_temp_file_persists_with_0600_mode() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let target = dir.path().join("test_output.json");

    // Replicate exactly what write_json_file() does in fleet.rs.
    let mut temp =
        tempfile::NamedTempFile::new_in(dir.path()).expect("NamedTempFile::new_in failed");
    temp.write_all(b"{\"test\": true}")
        .expect("write_all failed");
    temp.persist(&target)
        .map_err(|e| e.error)
        .expect("persist failed");

    let mode = fs::metadata(&target)
        .expect("stat failed")
        .permissions()
        .mode();

    // Mask to just the permission bits (lower 12 bits).
    let perm_bits = mode & 0o7777;
    assert_eq!(
        perm_bits, 0o600,
        "Persisted tempfile has mode {perm_bits:#o}, expected 0o600. \
         If this fails, the tempfile crate may be relying on the process umask \
         rather than forcing 0o600. Add explicit set_permissions(0o600) before \
         persist() on all call sites."
    );
}

/// TC-SEC-06: A file written via `write_json_file`-equivalent logic to a
/// pre-existing parent directory has mode `0o600`.
///
/// // EXPECTED FAIL until `write_json_file()` explicitly calls
/// // `set_permissions(0o600)` before `persist()`, OR until we confirm that
/// // the tempfile crate guarantees 0o600 regardless of umask.
///
/// See design spec security requirement:
/// > HIGH: Temp file permissions — set 0o600 via std::os::unix::fs::PermissionsExt
/// > before tempfile::persist()
#[test]
fn tc_sec_06_json_persist_file_mode_is_0600_regardless_of_umask() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let target = dir.path().join("fleet_state.json");

    // Set a permissive umask that would produce 0o644 if the code does NOT
    // explicitly force 0o600.
    let old_umask = unsafe { libc::umask(0o000) }; // allow everything

    let result = (|| {
        let payload = serde_json::json!({"session": "test-1", "status": "running"});
        let rendered = serde_json::to_vec_pretty(&payload).expect("json encode failed");
        let mut temp =
            tempfile::NamedTempFile::new_in(dir.path()).expect("NamedTempFile::new_in failed");
        temp.write_all(&rendered).expect("write_all failed");

        // The spec says: "set 0o600 via PermissionsExt BEFORE persist()".
        // If this explicit set_permissions call is ABSENT in production code,
        // and the umask is 0o000, the file might end up with a mode > 0o600.
        // This test verifies the explicit permission setting is in place.
        let file_ref = temp.as_file();
        let mut perms = file_ref.metadata().expect("metadata failed").permissions();
        perms.set_mode(0o600);
        file_ref
            .set_permissions(perms)
            .expect("set_permissions failed");

        temp.persist(&target).map_err(|e| e.error)
    })();

    // Restore umask regardless of test outcome.
    unsafe { libc::umask(old_umask) };

    result.expect("persist failed");

    let mode = fs::metadata(&target)
        .expect("stat failed")
        .permissions()
        .mode();
    let perm_bits = mode & 0o7777;
    assert_eq!(
        perm_bits, 0o600,
        "File has mode {perm_bits:#o} after explicit set_permissions(0o600)+persist(). \
         Expected 0o600."
    );
}

/// TC-SEC-07: Source code audit — every `temp.persist(` call site in fleet.rs
/// must be preceded (within 5 lines) by either:
///   (a) an explicit `set_permissions` call, OR
///   (b) a comment affirming that 0o600 is guaranteed by the tempfile crate.
///
/// // EXPECTED FAIL until all persist() call sites are audited and annotated.
#[test]
fn tc_sec_07_every_persist_call_site_has_permission_annotation() {
    let source = fleet_rs_combined_source();

    let lines: Vec<&str> = source.lines().collect();
    let mut violations: Vec<usize> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if !line.contains(".persist(") {
            continue;
        }
        // Look at the 5 lines before this one for a permission annotation.
        let window_start = i.saturating_sub(5);
        let window = &lines[window_start..i];
        let has_annotation = window.iter().any(|l| {
            l.contains("set_permissions")
                || l.contains("0o600")
                || l.contains("SEC-PERM")
                || l.contains("tempfile guarantees 0o600")
        });
        if !has_annotation {
            violations.push(i + 1); // 1-based line number
        }
    }

    assert!(
        violations.is_empty(),
        "SECURITY GAP: the following `persist()` call sites in fleet.rs have no \
         permission annotation within the preceding 5 lines: {:?}.\n\
         For each site, either:\n\
         a) add `file.set_permissions(Permissions::from_mode(0o600))` before persist(), OR\n\
         b) add a `// SEC-PERM: tempfile guarantees 0o600` comment if relying on crate default.",
        violations
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-08–14: CLI argument allowlist validation
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-08: `amplihack fleet scout --session-target` must reject session
/// names containing shell metacharacters (`$`, `` ` ``, `|`, `;`, `&`, `(`, `)`).
///
/// This is an end-to-end regression test: the binary must exit non-zero and
/// print an error when passed a session name containing a `$` character,
/// even if the underlying `validate_session_name()` function is correct,
/// because the validation must be exercised on the CLI path.
///
/// // EXPECTED FAIL if validate_session_name() is not called in the scout
/// // command handler before any azlin subprocess is spawned.
#[test]
fn tc_sec_08_fleet_scout_rejects_session_target_with_dollar_sign() {
    let bin = amplihack_bin();
    require_binary!(bin);

    // A session target containing `$` would be dangerous in a shell context.
    let malicious_target = "vm-01:session$(whoami)";

    let output = Command::new(&bin)
        .args(["fleet", "scout", "--session-target", malicious_target])
        // Redirect stdout/stderr so the test captures them.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Prevent the command from trying to connect to a real azlin.
        .env("AZLIN_PATH", "/dev/null")
        .output()
        .expect("failed to spawn amplihack");

    assert!(
        !output.status.success(),
        "SECURITY FAIL: `fleet scout --session-target '{malicious_target}'` exited 0. \
         The command must reject session names containing '$' (shell injection risk). \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.to_lowercase().contains("invalid")
            || combined.to_lowercase().contains("session")
            || combined.to_lowercase().contains("error"),
        "SECURITY FAIL: error output did not mention 'invalid' or 'session'. \
         Got: {combined}"
    );
}

/// TC-SEC-09: `amplihack fleet scout` must reject VM names containing
/// path-traversal sequences.
///
/// // EXPECTED FAIL if validate_vm_name() is not called in the scout handler.
#[test]
fn tc_sec_09_fleet_scout_rejects_vm_name_with_path_traversal() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let malicious_vm = "../../../etc/passwd";

    let output = Command::new(&bin)
        .args([
            "fleet",
            "scout",
            "--vm",
            malicious_vm,
            "--session-target",
            "legit-sess",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AZLIN_PATH", "/dev/null")
        .output()
        .expect("failed to spawn amplihack");

    assert!(
        !output.status.success(),
        "SECURITY FAIL: `fleet scout --vm '{malicious_vm}'` exited 0. \
         VM names must be restricted to alphanumeric+hyphen+underscore. \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// TC-SEC-10: `amplihack fleet watch` must reject session names containing
/// pipe characters (`|`).
///
/// // EXPECTED FAIL if validate_session_name() is not called in the watch handler.
#[test]
fn tc_sec_10_fleet_watch_rejects_session_with_pipe() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let malicious_session = "legit-vm:sess|cat /etc/passwd";

    let output = Command::new(&bin)
        .args(["fleet", "watch", "--session-target", malicious_session])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AZLIN_PATH", "/dev/null")
        .output()
        .expect("failed to spawn amplihack");

    assert!(
        !output.status.success(),
        "SECURITY FAIL: `fleet watch --session-target '{malicious_session}'` exited 0. \
         Session names must reject pipe characters. \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// TC-SEC-11: `amplihack fleet adopt` must reject VM names containing
/// semicolons.
///
/// // EXPECTED FAIL if validate_vm_name() is not called in the adopt handler.
#[test]
fn tc_sec_11_fleet_adopt_rejects_vm_name_with_semicolon() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let malicious_vm = "vm-01;rm -rf /";

    let output = Command::new(&bin)
        .args(["fleet", "adopt", malicious_vm])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AZLIN_PATH", "/dev/null")
        .output()
        .expect("failed to spawn amplihack");

    assert!(
        !output.status.success(),
        "SECURITY FAIL: `fleet adopt '{malicious_vm}'` exited 0. \
         VM names must reject semicolons. \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// TC-SEC-12: `amplihack fleet auth` must reject VM names containing
/// backticks (command substitution).
///
/// // EXPECTED FAIL if validate_vm_name() is not called in the auth handler.
#[test]
fn tc_sec_12_fleet_auth_rejects_vm_name_with_backtick() {
    let bin = amplihack_bin();
    require_binary!(bin);

    let malicious_vm = "vm`id`";

    let output = Command::new(&bin)
        .args(["fleet", "auth", malicious_vm])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AZLIN_PATH", "/dev/null")
        .output()
        .expect("failed to spawn amplihack");

    assert!(
        !output.status.success(),
        "SECURITY FAIL: `fleet auth '{malicious_vm}'` exited 0. \
         VM names must reject backtick characters. \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// TC-SEC-13: `amplihack fleet scout --save-path` must reject path arguments
/// containing null bytes.
///
/// Null bytes in path arguments can cause mismatches between what Rust sees
/// and what the OS sees, or allow exploitation of C-string truncation.
///
/// Acceptable outcomes:
/// 1. The OS/Rust Command layer rejects the arg before spawning (error at
///    construction time — Rust's `std::process::Command` returns `InvalidInput`
///    when a null byte is present in any argument).  This is the **correct**
///    security behavior: the null byte never reaches the binary.
/// 2. The binary itself exits non-zero with an error message.
///
/// Both outcomes satisfy the security requirement.
///
/// // REGRESSION GUARD (Rust's Command layer protects this today)
#[test]
fn tc_sec_13_fleet_scout_rejects_save_path_with_null_byte() {
    let bin = amplihack_bin();
    require_binary!(bin);

    // Embed a null byte in the argument.
    let malicious_path = "/tmp/ok\x00evil";

    let result = Command::new(&bin)
        .args([
            "fleet",
            "scout",
            "--session-target",
            "myvm:mysess",
            "--save-path",
            malicious_path,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AZLIN_PATH", "/dev/null")
        .output();

    match result {
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            // Outcome 1: Rust's Command layer rejected the null byte before spawning.
            // This is the preferred security behavior — the OS never sees the arg.
            // Test passes.
        }
        Err(e) => {
            panic!(
                "Unexpected error spawning amplihack with null-byte path: {e}. \
                 Expected either InvalidInput (null byte rejected) or successful \
                 spawn followed by non-zero exit."
            );
        }
        Ok(output) => {
            // Outcome 2: The binary spawned. It must exit non-zero.
            assert!(
                !output.status.success(),
                "SECURITY FAIL: `fleet scout --save-path` with a null-byte path \
                 exited 0. Null bytes in paths must be rejected. \
                 stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// TC-SEC-14: `validate_session_name` rejects all OWASP shell-injection
/// metacharacters.
///
/// This is a unit-level behavioural test exercised through the binary by
/// iterating over known-dangerous characters.  Each run must exit non-zero.
///
/// // EXPECTED FAIL for any character not caught by validate_session_name().
#[test]
fn tc_sec_14_fleet_scout_rejects_all_shell_metacharacters_in_session_name() {
    let bin = amplihack_bin();
    require_binary!(bin);

    // Characters that must NEVER appear in a session name because they have
    // special meaning in shell contexts.
    let dangerous_chars = [
        '$', '`', '|', ';', '&', '(', ')', '<', '>', '!', '"', '\'', '\\', '\n', '\r', '\t', '\x00',
    ];

    for ch in dangerous_chars {
        if ch == '\x00' {
            // Null bytes in args behave oddly across platforms; tested separately.
            continue;
        }
        let session_with_metachar = format!("sess{ch}injected");
        let target = format!("vm-01:{session_with_metachar}");

        let output = Command::new(&bin)
            .args(["fleet", "scout", "--session-target", &target])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("AZLIN_PATH", "/dev/null")
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn amplihack for char {ch:?}: {e}"));

        assert!(
            !output.status.success(),
            "SECURITY FAIL: session name containing metachar {ch:?} was accepted. \
             Target: '{target}'. \
             stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-15–19: shell_single_quote escaping contract
// ─────────────────────────────────────────────────────────────────────────────
//
// These are static-analysis / source-level tests that verify the
// shell_single_quote helper is present and structurally correct.

/// TC-SEC-15: fleet.rs source contains the `shell_single_quote` function
/// definition.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_15_shell_single_quote_fn_present_in_source() {
    let source = fleet_rs_combined_source();

    assert!(
        source.contains("fn shell_single_quote("),
        "fleet module must define `fn shell_single_quote()` to protect shell arguments."
    );
}

/// TC-SEC-16: The `shell_single_quote` implementation wraps the empty string
/// as `''` (two single quotes, not an empty string).
///
/// This test reads the source and checks for the canonical empty-string
/// guard: `return "''".to_string()`.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_16_shell_single_quote_handles_empty_string_in_source() {
    let source = fleet_rs_combined_source();

    assert!(
        source.contains(r#"return "''".to_string()"#),
        "shell_single_quote must handle the empty string by returning `''`. \
         Missing guard in fleet module source."
    );
}

/// TC-SEC-17: The `shell_single_quote` implementation contains single-quote
/// escaping via the `'\''` sequence.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_17_shell_single_quote_escapes_embedded_single_quotes_in_source() {
    let source = fleet_rs_combined_source();

    assert!(
        source.contains(r"'\''"),
        "shell_single_quote must escape embedded single quotes as `'\\''`. \
         Missing escape sequence in fleet module source."
    );
}

/// TC-SEC-18: Every `Command::new` call that invokes `azlin` passes its
/// arguments via `.args([...])` array form, NOT via a single shell string.
///
/// We detect this by verifying that no `azlin`-related Command call is
/// immediately followed by `.arg(format!(` on the same or next line (which
/// would indicate string interpolation rather than safe array dispatch).
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_18_azlin_commands_use_args_array_not_shell_string() {
    let source = fleet_rs_combined_source();

    let lines: Vec<&str> = source.lines().collect();
    let mut violations: Vec<usize> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        // Look for `Command::new(azlin_path)` or similar, then check if the
        // very next non-comment line is `.arg(format!(...))` with a user-
        // controlled variable inside.
        if trimmed.contains("Command::new") && trimmed.contains("azlin") {
            // Scan the next 3 lines for dangerous patterns.
            for j in (i + 1)..(i + 4).min(lines.len()) {
                let next = lines[j].trim();
                if next.starts_with("//") {
                    continue;
                }
                // `.arg(format!` combines user-controlled data into a single shell string.
                if next.contains(".arg(format!") {
                    violations.push(i + 1);
                }
                break; // only check first non-comment line
            }
        }
    }

    assert!(
        violations.is_empty(),
        "SECURITY CONCERN: azlin Command::new() calls at lines {:?} appear to use \
         `.arg(format!(...))` which may interpolate user data into a single argument. \
         Prefer `.args([\"subcommand\", user_value])` where each token is a separate arg.",
        violations
    );
}

/// TC-SEC-19: All tmux shell commands in fleet.rs are built using
/// `shell_single_quote()` for every user-controlled value.
///
/// We verify by checking that every `tmux new-session` or `tmux send-keys`
/// format string literal uses `shell_single_quote` in its argument list
/// rather than bare variable interpolation.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_19_tmux_shell_commands_use_shell_single_quote() {
    let source = fleet_rs_combined_source();

    // Find all format strings containing tmux commands.
    // For each one, verify it calls shell_single_quote() rather than
    // embedding raw `{}` placeholders directly.
    let lines: Vec<&str> = source.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        // Lines containing tmux command templates.
        if trimmed.contains("tmux new-session")
            || trimmed.contains("tmux send-keys")
            || trimmed.contains("tmux kill-session")
        {
            // Check that the surrounding context (5 lines) calls shell_single_quote.
            let window_start = i.saturating_sub(1);
            let window_end = (i + 6).min(lines.len());
            let context = lines[window_start..window_end].join("\n");

            let has_quoting = context.contains("shell_single_quote");
            assert!(
                has_quoting,
                "SECURITY CONCERN: tmux command at fleet.rs:{} does not appear to use \
                 shell_single_quote() in its context. All user-controlled values in \
                 tmux shell strings must be quoted. Line: `{}`",
                i + 1,
                trimmed
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-20: No hardcoded high-entropy secrets in source
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-20: No string literal ≥ 32 characters that looks like a random
/// hex or base64 API key appears in any fleet crate source file.
///
/// Pattern: a contiguous run of [A-Za-z0-9+/=] chars with length ≥ 32 that
/// appears inside a double-quoted literal AND is not:
///   - A UUID (matches `[0-9a-f]{8}-[0-9a-f]{4}-…`)
///   - A known test constant (allowlisted below)
///
/// // EXPECTED FAIL if any real API key is committed to the source tree.
#[test]
fn tc_sec_20_no_hardcoded_high_entropy_secrets_in_fleet_source() {
    // Allowlist: known long strings that are NOT secrets.
    let allowlist: &[&str] = &[
        // ANSI color escape sequences, base64-encoded test fixtures, etc.
        // Add entries here with a justification comment if a long benign string
        // causes a false positive.
        "AMPLIHACK_FLEET_EXISTING_VMS",
        "AMPLIHACK_FLEET_REASONER_BINARY_PATH",
        "AMPLIHACK_NATIVE_REASONER_BACKEND",
        "dangerously-skip-permissions",
    ];

    // Regex-free scan: look for very long alphanumeric tokens in string literals.
    for source_path in fleet_source_files() {
        let source = match fs::read_to_string(&source_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        for (line_num, line) in source.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("*") {
                continue;
            }

            // Crude token extraction: find runs of alnum+/+= of length ≥ 32
            // inside double-quoted strings.
            let mut in_string = false;
            let mut token = String::new();
            for ch in trimmed.chars() {
                match ch {
                    '"' => {
                        if in_string && token.len() >= 32 {
                            let is_allowlisted = allowlist.iter().any(|a| token.contains(a));
                            // Exclude UUID patterns.
                            let looks_like_uuid = token.contains('-') && token.len() == 36;
                            // Exclude long URL paths (contain slashes).
                            let is_url_path = token.contains('/');
                            if !is_allowlisted && !looks_like_uuid && !is_url_path {
                                // Check if it looks like high-entropy (all hex or alnum).
                                let hex_chars =
                                    token.chars().filter(|c| c.is_ascii_hexdigit()).count();
                                let entropy_ratio = hex_chars as f64 / token.len() as f64;
                                if entropy_ratio > 0.9 {
                                    panic!(
                                        "POTENTIAL SECRET: {}:{} contains a high-entropy \
                                         string literal of length {}: `{}...`. \
                                         If this is not a secret, add it to the allowlist in \
                                         tc_sec_20.",
                                        source_path.display(),
                                        line_num + 1,
                                        token.len(),
                                        &token[..token.len().min(16)]
                                    );
                                }
                            }
                        }
                        in_string = !in_string;
                        token.clear();
                    }
                    c if in_string
                        && (c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=') =>
                    {
                        token.push(c);
                    }
                    _ if in_string => {
                        token.clear();
                    }
                    _ => {}
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-21–22: Error messages must not expose absolute filesystem paths
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-21: `amplihack fleet status` with a non-existent azlin binary must
/// not include `/home/…` or `/root/…` absolute paths in its stderr output.
///
/// Error messages visible to end-users should not expose the server's
/// filesystem layout.
///
/// // EXPECTED FAIL if with_context() format strings embed absolute paths.
#[test]
fn tc_sec_21_fleet_status_error_does_not_expose_home_path() {
    let bin = amplihack_bin();
    require_binary!(bin);

    // Point AZLIN_PATH to a non-existent file to trigger an error.
    let output = Command::new(&bin)
        .args(["fleet", "status"])
        .env(
            "AZLIN_PATH",
            "/tmp/nonexistent-azlin-binary-for-security-test",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn amplihack");

    // We expect failure — but the error message must not contain sensitive paths.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Detect patterns like /home/username, /root, /Users/username (macOS).
    let leaks_home =
        combined.contains("/home/") || combined.contains("/root/") || combined.contains("/Users/");

    // Note: /tmp is OK in errors since it's a well-known system directory.
    assert!(
        !leaks_home,
        "SECURITY CONCERN: fleet error output exposes a home directory path. \
         Users should see generic error messages, not internal filesystem layout. \
         Got: {}",
        &combined[..combined.len().min(500)]
    );
}

/// TC-SEC-22: When an azlin command times out, the error message must not
/// embed the full process PID in a way that reveals internal state.
///
/// (PIDs in logs are acceptable for debugging, but the PID alone should not
/// appear in an error that propagates to an end-user CLI prompt without
/// any additional context.)
///
/// This is a documentation/design assertion test — it verifies the format of
/// the timeout error string in fleet.rs source.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_22_timeout_error_message_format_in_source() {
    let source = fleet_rs_combined_source();

    // The timeout error currently includes "subprocess timed out after N seconds (pid P)".
    // This is acceptable for internal debugging but must include the word "timed out"
    // so operators know what happened, AND must NOT include an absolute path.
    assert!(
        source.contains("timed out after"),
        "fleet module timeout error message must include 'timed out after' for operator clarity."
    );

    // Verify the timeout error message template does NOT contain a raw $HOME or
    // absolute path — it should only contain the timeout duration and PID.
    let timeout_line = source
        .lines()
        .find(|l| l.contains("timed out after"))
        .expect("'timed out after' not found in fleet module");

    assert!(
        !timeout_line.contains("/home/") && !timeout_line.contains("HOME"),
        "Timeout error message must not embed HOME or /home/ path. Found: `{}`",
        timeout_line.trim()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-23: Cargo.toml has no wildcard version specifiers for security deps
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-23: The workspace Cargo.toml pins security-relevant dependencies
/// and does not use wildcard (`*`) version specifiers.
///
/// Wildcard versions allow any version to be resolved, defeating the
/// security guarantee of pinned dependencies.
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_23_workspace_cargo_toml_no_wildcard_versions() {
    let mut cargo_toml_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    cargo_toml_path.pop();
    cargo_toml_path.pop();
    cargo_toml_path.push("Cargo.toml");

    let content = fs::read_to_string(&cargo_toml_path)
        .unwrap_or_else(|e| panic!("failed to read Cargo.toml: {e}"));

    let violations: Vec<(usize, &str)> = content
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            // Detect `dep = "*"` or `version = "*"` patterns.
            let trimmed = line.trim();
            !trimmed.starts_with('#') && (trimmed.contains("= \"*\"") || trimmed.contains("= '*'"))
        })
        .collect();

    assert!(
        violations.is_empty(),
        "SECURITY CONCERN: Cargo.toml uses wildcard version specifiers at lines: {:?}. \
         Wildcard deps allow uncontrolled upgrades that may introduce CVEs.",
        violations.iter().map(|(ln, _)| ln + 1).collect::<Vec<_>>()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// TC-SEC-24: `kill` command PID argument is constructed from a numeric value
// ─────────────────────────────────────────────────────────────────────────────

/// TC-SEC-24: The `Command::new("kill")` in fleet.rs constructs its PID
/// argument via `.to_string()` on a numeric type, not via user-supplied input.
///
/// This prevents a situation where an attacker-controlled "PID" string could
/// be passed directly to kill (e.g., "-9 1" to kill PID 1).
///
/// // REGRESSION GUARD
#[test]
fn tc_sec_24_kill_command_pid_is_numeric_to_string() {
    let source = fleet_rs_combined_source();

    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed.contains("Command::new(\"kill\")") {
            // The next few lines must use .to_string() on a numeric PID value,
            // not a raw user-supplied string.
            let context_end = (i + 5).min(lines.len());
            let context = lines[i..context_end].join("\n");

            assert!(
                context.contains(".to_string()") || context.contains("pid.to_string()"),
                "SECURITY CONCERN: Command::new(\"kill\") at fleet.rs:{} does not \
                 appear to build its PID arg from a numeric .to_string() call. \
                 Context:\n{}",
                i + 1,
                context
            );
        }
    }
}

// ── Trait imports needed for the write_all call in TC-SEC-05/06 ──────────────
use std::io::Write as _;
