//! Outside-in controls of the actual canonical command and its retained report.
mod controls;
mod native;

use super::step_command;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
};
use tempfile::TempDir;

const STEP: &str = "step-19c-zero-bs-verification";
const LABELS: [&str; 6] = [
    "Checking for TODO/FIXME comments",
    "Checking for stub indicators",
    "Rust: bail/panic stubs (zero-stubs policy)",
    "Shell: || true, >/dev/null 2>&1, set +e",
    "Rust: unwrap without context, expect without message",
    "TypeScript/JS: empty catch, catch return undefined",
];
const PATTERNS: [&str; 6] = [
    "(TODO|FIXME)",
    "(todo!|unimplemented!|panic!.*stub)",
    r#"bail!\("not implemented|bail!\("TODO|bail!\("unimplemented|panic!\("not implemented|panic!\("TODO"#,
    r"\|\| true|\|\| :|>/dev/null 2>&1|2>/dev/null|&>/dev/null|set \+e",
    r#"\.unwrap\(\)$|\.expect\(\"\"\)"#,
    r"catch.*\{.*\}$|catch.*return undefined|catch.*return \[\]",
];

pub(super) struct Fixture {
    temp: TempDir,
    shell: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("zero-bs-")
            .tempdir()
            .unwrap();
        assert!(
            temp.path().starts_with("/d0"),
            "private /d0 TMPDIR required"
        );
        let shell = std::env::var_os("AMPLIHACK_TEST_BASH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/bin/bash"));
        let f = Self { temp, shell };
        for dir in ["repo", "home", "tmp", "bin"] {
            fs::create_dir(f.temp.path().join(dir)).unwrap();
        }
        std::os::unix::fs::symlink(&f.shell, f.temp.path().join("bin/bash")).unwrap();
        assert!(f.git(&["init", "-q"]).status.success());
        f
    }
    fn repo(&self) -> PathBuf {
        self.temp.path().join("repo")
    }
    fn child(&self, tool: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(tool);
        c.env_clear()
            .current_dir(self.repo())
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.temp.path().join("bin").display()),
            )
            .env("HOME", self.temp.path().join("home"))
            .env("TMPDIR", self.temp.path().join("tmp"))
            .env("REPO_PATH", self.repo())
            .env("RUSTC_WRAPPER", "")
            .env("CARGO_BUILD_JOBS", "8");
        c
    }
    fn git(&self, args: &[&str]) -> Output {
        self.child("/usr/bin/git").args(args).output().unwrap()
    }
    fn run(&self) -> Output {
        self.run_with("", "")
    }
    fn run_with(&self, prefix: &str, failure: &str) -> Output {
        self.child(&self.shell)
            .args([
                "-c",
                &format!("{prefix}\n{}", step_command("workflow-pr-review", STEP)),
            ])
            .env("FAIL_TOOL", failure)
            .output()
            .unwrap()
    }
    fn write(&self, name: &str, data: &[u8]) {
        fs::write(self.repo().join(name), data).unwrap();
    }
    fn shim(&self, name: &str, body: &str) {
        let path = self.temp.path().join("bin").join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn large(&self) {
        let rs = format!(
            "// TODO {}\n todo!();\nbail!(\"not implemented\");\nf().unwrap()\n// TODO FINAL_MARKER\n",
            "x".repeat(220_000)
        );
        for (name, data) in [
            ("tracked.rs", rs.as_bytes()),
            ("tracked.sh", b"set +e\n"),
            ("tracked.js", b"catch(e) {}\n"),
        ] {
            self.write(name, data);
        }
        assert!(self.git(&["add", "."]).status.success());
        // Newline, command substitutions, backticks and literal bytes stay data.
        self.write("untracked\n$(touch PWNED)`touch PWNED`.rs", rs.as_bytes());
        self.write("untracked.sh", b"set +e\n");
        self.write("untracked.js", b"catch(e) {}\n");
        self.write(
            "hostile\nbytes.md",
            b"TODO \xff\x00 `touch PWNED` $(touch PWNED) FINAL_BINARY_MARKER\n",
        );
    }
}
fn receipt(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= 8192);
    serde_json::from_slice(
        output
            .stdout
            .strip_prefix(b"=== ZERO-BS SUMMARY ===\n")
            .expect("native String heading"),
    )
    .unwrap()
}
fn report(j: &Value) -> Vec<u8> {
    let path = PathBuf::from(j["report_path"].as_str().unwrap());
    assert!(path.is_absolute() && path.starts_with("/d0"));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let data = fs::read(path).unwrap();
    assert_eq!(data.len() as u64, j["report_bytes"].as_u64().unwrap());
    data
}
fn expected(f: &Fixture, index: usize, tracked: bool) -> Vec<u8> {
    let globs: &[&str] = match index {
        0 => &[
            "*.rs",
            "*.ts",
            "*.js",
            "*.sh",
            "*.bash",
            "*.md",
            "*.yaml",
            "*.yml",
            "Makefile",
            "Dockerfile",
        ],
        1 | 2 | 4 => &["*.rs"],
        3 => &["*.sh", "*.bash", "Makefile", "Dockerfile"],
        _ => &["*.ts", "*.js"],
    };
    let output = if tracked {
        f.child("/usr/bin/git")
            .args(["grep", "-n", "-E", "-a", "-z", PATTERNS[index], "--"])
            .args(globs)
            .output()
            .unwrap()
    } else {
        let files = f
            .child("/usr/bin/git")
            .args(["ls-files", "--others", "--exclude-standard", "-z", "--"])
            .args(globs)
            .output()
            .unwrap();
        assert!(files.status.success());
        use std::os::unix::ffi::OsStrExt;
        f.child("/usr/bin/grep")
            .args(["-n", "-E", "-H", "-a", "-Z", "--", PATTERNS[index]])
            .args(
                files
                    .stdout
                    .split(|b| *b == 0)
                    .filter(|p| !p.is_empty())
                    .map(std::ffi::OsStr::from_bytes),
            )
            .output()
            .unwrap()
    };
    assert_eq!(output.status.code(), Some(0));
    output.stdout
}

#[test]
fn large_report_exact_parity_counts_literal_bytes_and_final_marker() {
    let f = Fixture::new();
    f.large();
    let j = receipt(&f.run());
    let data = report(&j);
    assert!(data.len() > 202_307);
    assert_eq!(j["scan_complete"], true);
    assert_eq!(j["status"], "WARNING_REVIEW_REQUIRED");
    assert_eq!(j["findings_resolved"], false);
    assert_eq!(
        j["presentation"],
        serde_json::json!({"previews_omitted":true,"omitted_report_bytes":data.len(),"report_truncated":false,"summary_truncated":false,"stdout_max_bytes":8192})
    );
    assert!(!f.repo().join("PWNED").exists());
    let mut full=b"=== ZERO-BS VERIFICATION ===\nFormat: NUL-delimited filenames; tracked line numbers also NUL-delimited. Complete matching lines follow.\n".to_vec();
    for (i, label) in LABELS.iter().enumerate() {
        full.extend_from_slice(format!("\n--- {label} ---\n").as_bytes());
        full.extend(expected(&f, i, true));
        full.extend(expected(&f, i, false));
        let count = if i == 0 { 2 } else { 1 };
        let extra = if i == 0 { 1 } else { 0 };
        assert_eq!(j["categories"][i]["tracked_count"], count);
        assert_eq!(j["categories"][i]["untracked_count"], count + extra);
        assert_eq!(j["categories"][i]["count"], 2 * count + extra);
    }
    full.extend_from_slice(b"\n=== Zero-BS Verification Complete ===\n");
    assert_eq!(data, full);
}
