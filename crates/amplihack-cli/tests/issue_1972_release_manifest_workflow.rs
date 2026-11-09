//! Issue #1972 D8, D9: `release.yml` generates the bundle manifest once,
//! embeds the same bytes in every build leg and attaches them to the release.
//!
//! The workflow checks are text-based, like the issue_1356 and issue_1358
//! guards, so a failure names the line to edit and no YAML dependency is
//! needed. The post-build embed check is a script, run here against fixture
//! bytes and against this test binary's own rustc output.

use std::fs;
use std::path::{Path, PathBuf};

const PLACEHOLDER: &str = "crates/amplihack-cli/release/amplifier-bundle-manifest.txt";
const ASSET: &str = "amplifier-bundle-manifest.txt";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn workflow(name: &str) -> String {
    fs::read_to_string(repo_root().join(".github/workflows").join(name)).unwrap()
}

/// The text of one top-level job, from its `  name:` line to the next job.
fn job(body: &str, name: &str) -> String {
    let header = format!("  {name}:\n");
    let start = body
        .find(&header)
        .unwrap_or_else(|| panic!("release.yml has no `{name}` job"));
    let rest = &body[start + header.len()..];
    let end = rest
        .lines()
        .scan(0usize, |offset, line| {
            let at = *offset;
            *offset += line.len() + 1;
            Some((at, line))
        })
        .find(|(_, line)| {
            line.len() > 2
                && line.starts_with("  ")
                && !line.starts_with("   ")
                && line.trim_end().ends_with(':')
                && !line.trim_start().starts_with('#')
        })
        .map_or(rest.len(), |(at, _)| at);
    rest[..end].to_string()
}

fn position(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle:?} in:\n{haystack}"))
}

#[test]
fn issue_1972_the_manifest_job_is_read_only_and_runs_no_cargo() {
    let body = workflow("release.yml");
    let manifest = job(&body, "bundle-manifest");
    assert!(manifest.contains("needs: version-bump"), "{manifest}");
    assert!(manifest.contains("contents: read"), "{manifest}");
    assert!(!manifest.contains("contents: write"), "{manifest}");
    assert!(
        manifest.contains("persist-credentials: false"),
        "{manifest}"
    );
    assert!(manifest.contains("ref: ${{ needs.version-bump.outputs.sha }}"));
    assert!(manifest.contains("scripts/generate-bundle-manifest.sh amplifier-bundle"));
    assert!(
        manifest.contains("sha256sum --strict --quiet -c"),
        "self-check"
    );
    assert!(manifest.contains("sha256: ${{ steps.generate.outputs.sha256 }}"));
    assert!(manifest.contains("name: amplifier-bundle-manifest"));
    for forbidden in ["cargo ", "RUSTC_WRAPPER", "sccache", "secrets."] {
        assert!(
            !manifest.contains(forbidden),
            "{forbidden:?} in:\n{manifest}"
        );
    }
    for line in manifest.lines().filter(|l| l.contains("uses:")) {
        let pin = line.split('@').nth(1).unwrap_or_default();
        assert!(
            pin.len() >= 40 && pin[..40].chars().all(|c| c.is_ascii_hexdigit()),
            "actions must be pinned by SHA: {line}"
        );
    }
}

#[test]
fn issue_1972_every_build_leg_embeds_the_verified_manifest_before_cargo_build() {
    let body = workflow("release.yml");
    let build = job(&body, "build");
    assert!(
        build.contains("needs: [version-bump, bundle-manifest]"),
        "{build}"
    );
    let download = position(&build, "name: amplifier-bundle-manifest");
    let digest = position(
        &build,
        "MANIFEST_SHA256: ${{ needs.bundle-manifest.outputs.sha256 }}",
    );
    let format = position(&build, "=~ ^[0-9a-f]{64}$");
    let copy = position(&build, &format!("\"${{MANIFEST}}\" {PLACEHOLDER}"));
    let cargo = position(&build, "cargo build --release");
    assert!(download < copy && digest < copy && format < copy && copy < cargo);
}

#[test]
fn issue_1972_every_build_leg_checks_its_binary_carries_the_manifest_before_packaging() {
    let body = workflow("release.yml");
    let build = job(&body, "build");
    let cargo = position(&build, "cargo build --release");
    let check = position(&build, "bash scripts/check-embedded-bundle-manifest.sh");
    let package = position(&build, "name: Package");
    assert!(cargo < check && check < package, "{build}");
    assert!(
        build
            .contains("MANIFEST: ${{ runner.temp }}/bundle-manifest/amplifier-bundle-manifest.txt")
    );
    assert!(build.contains("BINARY: target/${{ matrix.target }}/release/amplihack"));
}

// ── scripts/check-embedded-bundle-manifest.sh ──

const CHECK_SCRIPT: &str = "scripts/check-embedded-bundle-manifest.sh";

/// Three manifest lines in the generator's format.
const MANIFEST: &str = "\
0000000000000000000000000000000000000000000000000000000000000001  agents/a.md
0000000000000000000000000000000000000000000000000000000000000002  recipes/b.yaml
0000000000000000000000000000000000000000000000000000000000000003  tools/c.sh
";

/// Compiled into this test binary the way `include_str!` compiles the release
/// manifest into `amplihack`: as one `&'static str`. Distinct from `MANIFEST`,
/// so a match can only come from this constant.
const COMPILED_IN: &str = "\
abababababababababababababababababababababababababababababababab01  agents/compiled.md
abababababababababababababababababababababababababababababababab02  tools/compiled.sh
";

fn run_check(manifest: &[u8], binary: &[u8]) -> std::process::Output {
    let temp = tempfile::tempdir().unwrap();
    let manifest_path = temp.path().join("manifest.txt");
    let binary_path = temp.path().join("amplihack");
    fs::write(&manifest_path, manifest).unwrap();
    fs::write(&binary_path, binary).unwrap();
    run_check_paths(&manifest_path, &binary_path)
}

fn run_check_paths(manifest: &Path, binary: &Path) -> std::process::Output {
    std::process::Command::new("bash")
        .arg(repo_root().join(CHECK_SCRIPT))
        .arg(manifest)
        .arg(binary)
        .output()
        .unwrap()
}

/// Bytes on both sides, as a linker leaves them around a string constant.
fn embedded(text: &str) -> Vec<u8> {
    let mut bytes = vec![0u8, 0x7f, b'E', b'L', b'F', 0xff, 0x10];
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(&[0u8, 0xfe, 0x01]);
    bytes
}

#[cfg(unix)]
#[test]
fn issue_1972_the_embed_check_accepts_a_binary_with_every_line() {
    let output = run_check(MANIFEST.as_bytes(), &embedded(MANIFEST));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn issue_1972_the_embed_check_rejects_a_dev_build_with_the_empty_placeholder() {
    let output = run_check(MANIFEST.as_bytes(), &embedded(""));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("carries 0 of the 3 manifest lines"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn issue_1972_the_embed_check_rejects_a_stale_or_partial_embed() {
    let stale = MANIFEST.replace("tools/c.sh", "tools/old.sh");
    let output = run_check(MANIFEST.as_bytes(), &embedded(&stale));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("carries 2 of the 3"), "{stderr}");
    assert!(stderr.contains("first missing: 0000"), "{stderr}");
    assert!(stderr.contains("tools/c.sh"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn issue_1972_the_embed_check_rejects_an_empty_or_blank_line_manifest() {
    // A blank pattern would match every binary.
    for manifest in ["", "\n", "0000  agents/a.md\n\n"] {
        let output = run_check(manifest.as_bytes(), &embedded(MANIFEST));
        assert_eq!(output.status.code(), Some(1), "{manifest:?}");
    }
}

#[cfg(unix)]
#[test]
fn issue_1972_the_embed_check_finds_a_constant_rustc_compiled_into_a_real_binary() {
    // The check rests on a `&'static str` reaching the binary byte for byte.
    // This test binary is real rustc output, built in the same way.
    assert!(!std::hint::black_box(COMPILED_IN).is_empty());
    let temp = tempfile::tempdir().unwrap();
    let manifest = temp.path().join("manifest.txt");
    fs::write(&manifest, COMPILED_IN).unwrap();
    let output = run_check_paths(&manifest, &std::env::current_exe().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn issue_1972_the_release_attaches_the_same_bytes_without_a_checksum_sibling() {
    let body = workflow("release.yml");
    let release = job(&body, "release");
    assert!(
        release.contains("needs: [version-bump, bundle-manifest, build]"),
        "{release}"
    );
    assert!(release.contains("MANIFEST_SHA256: ${{ needs.bundle-manifest.outputs.sha256 }}"));
    assert!(release.contains("=~ ^[0-9a-f]{64}$"));
    assert!(release.contains(&format!("artifacts/amplifier-bundle-manifest/{ASSET}")));
    assert!(release.contains(&format!("release-assets/{ASSET}")));
    assert!(!release.contains(&format!("{ASSET}.sha256")), "{release}");
    // R4: the stamp commit may only touch the three manifests.
    assert!(release.contains("only the three manifests may be committed"));
    assert!(release.contains("Issue #1972 R4"));
}

#[test]
fn issue_1972_digests_reach_shell_code_only_through_env() {
    let body = workflow("release.yml");
    for line in body.lines() {
        if line.contains("needs.bundle-manifest.outputs.sha256") {
            let trimmed = line.trim_start();
            assert!(
                trimmed.starts_with("MANIFEST_SHA256:") || trimmed.starts_with("sha256:"),
                "the digest must be passed via env:, not interpolated into a script: {line}"
            );
        }
    }
}

#[test]
fn issue_1972_snapshots_never_embed_a_manifest() {
    let snapshot = workflow("publish-snapshot.yml");
    assert!(!snapshot.contains(PLACEHOLDER));
    assert!(!snapshot.contains("amplifier-bundle-manifest"));
}

#[test]
fn issue_1972_the_placeholder_is_committed_empty() {
    let placeholder = repo_root().join(PLACEHOLDER);
    assert_eq!(fs::metadata(&placeholder).unwrap().len(), 0);
}
