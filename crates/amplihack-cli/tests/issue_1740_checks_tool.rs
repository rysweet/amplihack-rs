//! Issue #1740: `amplifier-bundle/tools/quick_change_checks.sh` records each
//! check as a command and exit code, records a check that cannot run as
//! skipped (never as passed), and keeps check output and tokens out of what
//! it reports.
//!
//! The tool runs under `bash` started by
//! [`crate::isolated_amplihack::isolated`] with a temp `HOME`. `PATH` is a
//! directory of links to `/usr/bin` and `/bin` with `cargo`, `pre-commit`,
//! `npm` and `node` left out, so the skip cases do not depend on the host,
//! followed by a stub `npm` whose exit code per script is set by
//! `STUB_RC_<script>`.

use std::path::PathBuf;
use std::process::Output;

use crate::support::quick_change_repo::git;

const LEFT_OUT: &[&str] = &[
    "cargo",
    "rustc",
    "rustup",
    "pre-commit",
    "npm",
    "npx",
    "node",
];

fn tool() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/quick_change_checks.sh")
}

struct Fixture {
    tmp: tempfile::TempDir,
    wt: PathBuf,
    base: String,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "main"]);
        for (rel, body) in files {
            std::fs::write(wt.join(rel), body).unwrap();
        }
        std::fs::write(wt.join("README.md"), "# demo\n").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-q", "-m", "base"]);
        let base = git(&wt, &["rev-parse", "HEAD"]);
        std::fs::write(wt.join("README.md"), "# demo, edited\n").unwrap();
        Fixture { tmp, wt, base }
    }

    fn bin_dir(&self, with_npm: bool) -> PathBuf {
        let bin = self
            .tmp
            .path()
            .join(if with_npm { "bin-npm" } else { "bin" });
        std::fs::create_dir(&bin).unwrap();
        for dir in ["/usr/bin", "/bin"] {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let link = bin.join(&name);
                if LEFT_OUT.contains(&name.to_string_lossy().as_ref()) || link.exists() {
                    continue;
                }
                let _ = std::os::unix::fs::symlink(entry.path(), link);
            }
        }
        if with_npm {
            let record = self.tmp.path().join("npm-record");
            let stub = format!(
                "#!/bin/sh\nname=\"$1\"; [ \"$1\" = run ] && name=\"$2\"\n\
                 printf '%s GH_TOKEN=%s ANTHROPIC_API_KEY=%s\\n' \"$name\" \"${{GH_TOKEN:-unset}}\" \
                 \"${{ANTHROPIC_API_KEY:-unset}}\" >> '{}'\n\
                 echo STUB-OUTPUT-$name\nrc=$(printenv \"STUB_RC_$name\")\nexit \"${{rc:-0}}\"\n",
                record.display()
            );
            crate::support::exec_stub::write_executable(&bin.join("npm"), stub, 0o755);
        }
        bin
    }

    fn run(&self, with_npm: bool, env: &[(&str, &str)]) -> (Output, String) {
        let table = self.tmp.path().join("table.md");
        let mut c = crate::isolated_amplihack::isolated("/bin/bash");
        c.arg(tool())
            .arg(&self.wt)
            .arg(&self.base)
            .arg(&table)
            .env("HOME", self.tmp.path())
            .env("PATH", self.bin_dir(with_npm));
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().expect("run quick_change_checks.sh");
        let table = std::fs::read_to_string(&table).unwrap_or_default();
        (out, table)
    }

    fn npm_record(&self) -> String {
        std::fs::read_to_string(self.tmp.path().join("npm-record")).unwrap_or_default()
    }
}

fn summary(o: &Output) -> String {
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert_eq!(
        o.status.code(),
        Some(0),
        "stdout:\n{out}\nstderr:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(out.lines().count(), 1, "stdout must be one line: {out:?}");
    out.trim_end().to_string()
}

fn has_row(table: &str, command: &str, result: &str) -> bool {
    table
        .lines()
        .any(|l| l.contains(&format!("`{command}`")) && l.contains(result))
}

const PKG: &str = r#"{"name":"demo","scripts":{"lint":"x","test":"y"}}"#;

#[test]
fn passing_checks_are_recorded_with_exit_codes() {
    let f = Fixture::new(&[("package.json", PKG)]);
    let (o, table) = f.run(true, &[]);
    assert_eq!(
        summary(&o),
        "QUICK_CHANGE_CHECKS=passed ran=2 failed=0 skipped=0"
    );
    assert!(has_row(&table, "npm run lint", "exit 0"), "{table}");
    assert!(has_row(&table, "npm test", "exit 0"), "{table}");
}

#[test]
fn a_failing_check_is_recorded_and_does_not_stop_the_others() {
    let f = Fixture::new(&[("package.json", PKG)]);
    let (o, table) = f.run(true, &[("STUB_RC_lint", "7")]);
    assert_eq!(
        summary(&o),
        "QUICK_CHANGE_CHECKS=failed ran=2 failed=1 skipped=0"
    );
    assert!(has_row(&table, "npm run lint", "exit 7"), "{table}");
    assert!(has_row(&table, "npm test", "exit 0"), "{table}");
}

#[test]
fn a_missing_script_is_skipped_not_passed() {
    let f = Fixture::new(&[("package.json", r#"{"name":"demo","scripts":{"test":"y"}}"#)]);
    let (o, table) = f.run(true, &[]);
    assert_eq!(
        summary(&o),
        "QUICK_CHANGE_CHECKS=passed ran=1 failed=0 skipped=1"
    );
    assert!(
        has_row(&table, "npm run lint", "skipped (no \"lint\" script)"),
        "{table}"
    );
    assert!(
        !f.npm_record().contains("lint"),
        "a skipped script must not run"
    );
}

#[test]
fn missing_tools_are_skipped_with_their_reason() {
    let f = Fixture::new(&[
        ("package.json", PKG),
        (".pre-commit-config.yaml", "repos: []\n"),
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        ),
    ]);
    let (o, table) = f.run(false, &[]);
    let line = summary(&o);
    let skipped: u32 = line
        .strip_prefix("QUICK_CHANGE_CHECKS=none ran=0 failed=0 skipped=")
        .unwrap_or_else(|| panic!("{line}"))
        .parse()
        .unwrap();
    assert!(skipped >= 4, "{line}");
    for (cmd, reason) in [
        ("cargo fmt --all -- --check", "skipped (cargo not on PATH)"),
        (
            "pre-commit run --files README.md",
            "skipped (pre-commit not on PATH)",
        ),
        ("npm run lint", "skipped (npm not on PATH)"),
        ("npm test", "skipped (npm not on PATH)"),
    ] {
        assert!(
            has_row(&table, cmd, reason),
            "missing row {cmd} | {reason}:\n{table}"
        );
    }
    assert!(
        !table.contains("exit 0"),
        "a skipped check must never read as a pass"
    );
}

#[test]
fn nothing_applicable_gives_none() {
    let f = Fixture::new(&[]);
    let (o, table) = f.run(true, &[]);
    assert_eq!(
        summary(&o),
        "QUICK_CHANGE_CHECKS=none ran=0 failed=0 skipped=0"
    );
    assert!(table.trim().is_empty(), "{table}");
}

#[test]
fn check_output_stays_on_stderr_and_tokens_are_removed() {
    let f = Fixture::new(&[("package.json", PKG)]);
    let (o, table) = f.run(
        true,
        &[("GH_TOKEN", "ghp_secret"), ("ANTHROPIC_API_KEY", "sk-x")],
    );
    let line = summary(&o);
    assert!(!line.contains("STUB-OUTPUT") && !table.contains("STUB-OUTPUT"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("STUB-OUTPUT-lint"));
    let record = f.npm_record();
    assert!(
        record.contains("lint GH_TOKEN=unset ANTHROPIC_API_KEY=unset"),
        "{record}"
    );
    assert!(
        !record.contains("ghp_secret") && !record.contains("sk-x"),
        "{record}"
    );
}

#[test]
fn usage_errors_exit_2() {
    let f = Fixture::new(&[]);
    for args in [
        vec![],
        vec![f.wt.as_os_str()],
        vec![f.wt.as_os_str(), "nothex".as_ref()],
    ] {
        let mut c = crate::isolated_amplihack::isolated("/bin/bash");
        c.arg(tool()).args(&args).env("HOME", f.tmp.path());
        let o = c.output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
}
