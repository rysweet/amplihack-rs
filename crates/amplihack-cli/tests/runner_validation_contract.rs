//! Behavioral contracts for the private Cargo-only runner validation boundary.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};

fn cli_binary() -> std::path::PathBuf {
    std::env::var_os("AMPLIHACK_TEST_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("amplihack")
        })
}

const REPO: &str = "https://github.com/rysweet/amplihack-recipe-runner";
const REV: &str = "1111111111111111111111111111111111111111";
const REPORT: &str = r#"{"capabilities":["future","codex_exec"],"version":"custom-99","schema_version":1,"extra":true}"#;

struct Fixture {
    root: tempfile::TempDir,
}
impl Fixture {
    fn new(report: &str, status: i32) -> Self {
        let f = Self {
            root: tempfile::tempdir().unwrap(),
        };
        fs::create_dir(f.path().join("bin")).unwrap();
        fs::write(f.path().join("report"), report).unwrap();
        fs::write(f.binary(), format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'/calls\n/bin/cat '{}'/report\nprintf 'secret-canary' >&2\nexit {status}\n", f.path().display(), f.path().display())).unwrap();
        fs::set_permissions(f.binary(), fs::Permissions::from_mode(0o700)).unwrap();
        f
    }
    fn path(&self) -> &Path {
        self.root.path()
    }
    fn binary(&self) -> std::path::PathBuf {
        self.path().join("bin/recipe-runner-rs")
    }
    fn receipt(&self, entries: Value) {
        fs::write(
            self.path().join(".crates2.json"),
            serde_json::to_vec(&json!({"installs": entries})).unwrap(),
        )
        .unwrap();
    }
    fn valid_receipt(&self) {
        self.receipt(json!({format!("recipe-runner-rs 0.4.0 (git+{REPO}?rev={REV}#{REV})"): {"bins":["recipe-runner-rs"]}}));
    }
    fn run(&self, provider: &str, managed: bool, binary: &Path) -> Output {
        let mut c = Command::new(cli_binary());
        c.args([
            "internal",
            "validate-recipe-runner",
            "--provider",
            provider,
            "--binary",
        ])
        .arg(binary)
        .env("HOME", self.path())
        .env("PATH", self.path().join("bin"))
        .env_remove("RECIPE_RUNNER_RS_PATH")
        .env_remove("CODEX_HOME");
        if managed {
            c.arg("--cargo-home")
                .arg(self.path())
                .args(["--repository", REPO, "--revision", REV]);
        }
        c.output().unwrap()
    }
    fn success(&self, o: Output, provider: &str, kind: &str) {
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["provider"], provider);
        assert_eq!(v["provenance"]["kind"], kind);
        assert_eq!(v["runner"]["version"], "custom-99");
        assert_eq!(
            fs::read_to_string(self.path().join("calls")).unwrap(),
            "--capabilities\n"
        );
        assert!(
            !self.path().join(".amplihack").exists(),
            "validator must be read-only"
        );
    }
}
fn failure(o: Output, code: &str) {
    assert_eq!(o.status.code(), Some(1));
    assert!(
        o.stdout.is_empty(),
        "failed validation must not publish success JSON"
    );
    let v: Value = serde_json::from_slice(&o.stderr).expect("structured private endpoint error");
    assert_eq!(v["error"]["code"], code);
    assert!(!String::from_utf8_lossy(&o.stderr).contains("secret-canary"));
}
#[test]
fn reordered_additive_report_works_without_python_or_node() {
    let f = Fixture::new(REPORT, 0);
    f.success(f.run("codex", false, &f.binary()), "codex", "explicit");
}
#[test]
fn empty_capabilities_are_legacy_provider_compatible_only() {
    for provider in ["claude", "copilot", "codex"] {
        let f = Fixture::new(
            r#"{"capabilities":[],"schema_version":1,"version":"custom-99"}"#,
            0,
        );
        let o = f.run(provider, false, &f.binary());
        if provider == "codex" {
            failure(o, "RUNNER_CAPABILITY_MISSING");
        } else {
            f.success(o, provider, "explicit");
        }
    }
}
#[test]
fn ambiguous_malformed_and_wrong_typed_reports_fail() {
    for report in [
        r#"{"schema_version":1,"schema_version":1,"version":"v","capabilities":["codex_exec"]}"#,
        r#"{"schema_version":true,"version":"v","capabilities":[]}"#,
        r#"{"schema_version":1,"version":" ","capabilities":[]}"#,
        r#"{"schema_version":1,"version":"v","capabilities":[1]}"#,
        &format!("{REPORT} {{}}"),
        "not json",
    ] {
        let f = Fixture::new(report, 0);
        failure(f.run("claude", false, &f.binary()), "RUNNER_REPORT_INVALID");
    }
}
#[test]
fn failed_probe_cannot_publish_valid_report_or_raw_stderr() {
    let f = Fixture::new(REPORT, 17);
    failure(f.run("codex", false, &f.binary()), "RUNNER_PROBE_FAILED");
}
#[test]
fn managed_custom_cargo_home_requires_one_matching_receipt() {
    let f = Fixture::new(REPORT, 0);
    f.valid_receipt();
    f.success(f.run("codex", true, &f.binary()), "codex", "managed");
}
#[test]
fn receipt_identity_cannot_be_composed_across_entries() {
    let f = Fixture::new(REPORT, 0);
    f.receipt(json!({
        format!("unrelated 0.4.0 (git+{REPO}#{REV})"): {"bins":["unrelated"]},
        format!("recipe-runner-rs 0.4.0 (git+{REPO}#2222222222222222222222222222222222222222)"): {"bins":["recipe-runner-rs"]}
    }));
    failure(
        f.run("codex", true, &f.binary()),
        "RUNNER_PROVENANCE_MISMATCH",
    );
}
#[test]
fn missing_receipt_and_shadowed_selection_fail_visibly() {
    let f = Fixture::new(REPORT, 0);
    failure(f.run("codex", true, &f.binary()), "RUNNER_RECEIPT_MISSING");
    f.valid_receipt();
    let shadow = f.path().join("shadow");
    fs::copy(f.binary(), &shadow).unwrap();
    failure(f.run("codex", true, &shadow), "RUNNER_BINARY_SHADOWED");
}
#[test]
fn partial_managed_arguments_fail_before_probe() {
    let f = Fixture::new(REPORT, 0);
    let o = Command::new(cli_binary())
        .args([
            "internal",
            "validate-recipe-runner",
            "--provider",
            "codex",
            "--binary",
        ])
        .arg(f.binary())
        .arg("--cargo-home")
        .arg(f.path())
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(o.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&o.stderr);
    assert!(
        !diagnostic.contains("unrecognized subcommand"),
        "endpoint must exist: {diagnostic}"
    );
    assert!(
        diagnostic.contains("revision") || diagnostic.contains("INVALID_VALIDATION_REQUEST"),
        "{diagnostic}"
    );
    assert!(!f.path().join("calls").exists());
}

#[test]
fn invalid_utf8_and_oversized_reports_fail_with_bounded_diagnostics() {
    let f = Fixture::new(REPORT, 0);
    fs::write(f.path().join("report"), [0xff, 0xfe]).unwrap();
    failure(f.run("claude", false, &f.binary()), "RUNNER_REPORT_INVALID");
    fs::write(f.path().join("report"), vec![b'x'; 80_000]).unwrap();
    let output = f.run("claude", false, &f.binary());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.len() < 1024);
}

#[test]
fn repository_prefix_and_wrong_receipt_binary_cannot_establish_provenance() {
    let f = Fixture::new(REPORT, 0);
    for (repository, binary) in [
        (format!("{REPO}-foreign"), "recipe-runner-rs"),
        (REPO.to_owned(), "other"),
    ] {
        f.receipt(
            json!({format!("recipe-runner-rs 0.4.0 (git+{repository}#{REV})"): {"bins":[binary]}}),
        );
        failure(
            f.run("codex", true, &f.binary()),
            "RUNNER_PROVENANCE_MISMATCH",
        );
    }
    assert!(!f.path().join("calls").exists());
}

#[test]
fn exited_probe_with_inherited_pipe_is_bounded_and_cleans_its_group() {
    let f = Fixture::new(REPORT, 0);
    fs::write(
        f.binary(),
        format!(
            "#!/bin/sh\n/bin/sleep 60 &\necho $! > '{}/descendant'\n/bin/cat '{}/report'\n",
            f.path().display(),
            f.path().display()
        ),
    )
    .unwrap();
    let started = std::time::Instant::now();
    failure(f.run("codex", false, &f.binary()), "RUNNER_PROBE_FAILED");
    assert!(started.elapsed() < std::time::Duration::from_secs(13));
    let pid = fs::read_to_string(f.path().join("descendant")).unwrap();
    #[cfg(target_os = "linux")]
    if let Ok(stat) = fs::read_to_string(format!("/proc/{}/stat", pid.trim())) {
        let state = stat
            .rsplit_once(") ")
            .unwrap()
            .1
            .split_whitespace()
            .next()
            .unwrap();
        assert!(
            state == "Z" || state == "X",
            "probe descendant still running"
        );
    }
}
