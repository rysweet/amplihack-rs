//! Behavioural contract for #2441. Run the shipped evidence script and the
//! Cargo-built CLI, never a replacement executor. All observations follow
//! process completion; no polling, deadlines or source-text assertions.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use serde_json::Value;

use crate::isolated_amplihack::{self, PROFILE_ROOT_ENV};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Compile once, sharing only the immutable executable between test cases.
fn recorder() -> &'static Path {
    static RECORDER: OnceLock<PathBuf> = OnceLock::new();
    RECORDER.get_or_init(|| {
        let bin = isolated_amplihack::shared_temp_home().join("qa-arg-recorder");
        let out = isolated_amplihack::isolated("rustc")
            .arg("--edition=2021")
            .arg(workspace().join("tests/integration/fixtures/autodrive_qa_arg_recorder.rs"))
            .arg("-o")
            .arg(&bin)
            .output()
            .unwrap();
        assert!(out.status.success(), "recorder compilation: {out:?}");
        bin
    })
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    records: PathBuf,
}

impl Fixture {
    fn new(manifest: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let f = Self {
            repo: root.join("repo"),
            home: root.join("home"),
            bin: root.join("bin"),
            records: root.join("records"),
            root,
            _tmp: tmp,
        };
        for dir in [&f.repo, &f.home, &f.bin, &f.records] {
            fs::create_dir_all(dir).unwrap();
        }
        let proc_root = f.root.join("fake-proc");
        fs::create_dir(&proc_root).unwrap();
        fs::set_permissions(&proc_root, fs::Permissions::from_mode(0o700)).unwrap();
        isolated_amplihack::seed_proc_tree(&proc_root);
        symlink(env!("CARGO_BIN_EXE_amplihack"), f.bin.join("amplihack")).unwrap();
        symlink(recorder(), f.bin.join("qa-arg-recorder")).unwrap();
        symlink(recorder(), f.bin.join("recorder with spaces")).unwrap();
        // Detected commands are observations too; they must never start a real suite.
        symlink(recorder(), f.bin.join("cargo")).unwrap();
        f.write(
            "Cargo.toml",
            "[package]\nname = \"qa-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        f.write("src/main.rs", "fn main() {}\n");
        if let Some(manifest) = manifest {
            f.write(".amplihack/qa.toml", manifest);
        }
        f.git(&["init", "-q", "-b", "main"]);
        f.commit();
        f.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // A real non-doc change and a stable merge-base manifest are needed by
        // scope selection and qa.toml validation, even for an env override.
        f.write("src/main.rs", "fn main() { println!(\"changed\"); }\n");
        f.commit();
        f
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn child(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        assert!(self.home.starts_with(&self.root));
        assert_ne!(
            Some(self.home.as_os_str()),
            std::env::var_os("HOME").as_deref()
        );
        let mut c = isolated_amplihack::isolated(program);
        c.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env(PROFILE_ROOT_ENV, self.home.join("profiles"))
            .env("CODEX_PROFILES_ROOT", self.home.join("codex-profiles"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("AMPLIHACK_HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join(".gitconfig"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display()),
            )
            .env("QA_RECORD_DIR", &self.records)
            .env(
                isolated_amplihack::PROC_ROOT_ENV,
                self.root.join("fake-proc"),
            )
            .env("TMPDIR", &self.root);
        // isolated() retains the home guard, disables profile autostart and
        // supplies a private fake process table. Do not clear those here.
        amplihack_git::scrub(&mut c);
        c
    }

    fn git(&self, args: &[&str]) -> String {
        let mut c = amplihack_git::command_in(&self.repo);
        c.env_clear();
        isolated_amplihack::no_git_prompt(&mut c);
        let out = c
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join(".gitconfig"))
            .env("GIT_AUTHOR_NAME", "QA fixture")
            .env("GIT_AUTHOR_EMAIL", "qa@example.invalid")
            .env("GIT_COMMITTER_NAME", "QA fixture")
            .env("GIT_COMMITTER_EMAIL", "qa@example.invalid")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn commit(&self) {
        self.git(&["add", "-A"]);
        // Scratch history through plumbing: no repository hooks are installed
        // in this fixture, and no developer commit hooks are bypassed.
        let tree = self.git(&["write-tree"]);
        let mut args = vec!["commit-tree", tree.as_str(), "-m", "fixture"];
        if self.repo.join(".git/refs/heads/main").exists() {
            args.extend(["-p", "HEAD"]);
        }
        let commit = self.git(&args);
        self.git(&["update-ref", "HEAD", &commit]);
    }

    fn helper(&self, command: Option<&str>, dir: &Path) -> Output {
        let mut c = self.child(env!("CARGO_BIN_EXE_amplihack"));
        c.args(["orch", "helper", "autodrive-qa-command"])
            .arg(format!("--dir={}", dir.display()));
        if let Some(command) = command {
            c.env("AUTODRIVE_QA_COMMAND", command);
        }
        let out = c.output().unwrap();
        isolated_amplihack::assert_proc_root_honoured(&String::from_utf8_lossy(&out.stderr));
        out
    }

    fn evidence_child(&self) -> Command {
        let mut child = self.child("bash");
        child
            .arg(workspace().join("amplifier-bundle/tools/autodrive_qa_evidence.sh"))
            .env("REPO_PATH", &self.repo)
            .env("AUTODRIVE_QA_EVIDENCE", self.root.join("evidence.json"))
            .env("AUTODRIVE_QA_BASE", "main")
            .env("AUTODRIVE_ROUND_LABEL", "issue-2441");
        child
    }

    fn evidence(&self, env: &[(&str, &str)]) -> (Value, String) {
        let out = self
            .evidence_child()
            .envs(env.iter().copied())
            .output()
            .unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(out.status.success(), "evidence generation: {stderr}");
        isolated_amplihack::assert_proc_root_honoured(&stderr);
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            stdout.trim_end(),
            fs::read_to_string(self.root.join("evidence.json")).unwrap()
        );
        let value: Value = serde_json::from_str(stdout.trim()).expect("one JSON evidence record");
        assert_eq!(value["head_sha"], self.git(&["rev-parse", "HEAD"]));
        assert_eq!(value["qa_round"], "issue-2441");
        assert_eq!(value["gadugi_status"], "NOT_REQUIRED");
        (value, stderr)
    }

    fn record(&self, tag: &str, dir: &Path, args: &[&str]) {
        let bytes = fs::read(self.records.join(tag))
            .unwrap_or_else(|e| panic!("missing record {tag}: {e}"));
        let fields: Vec<_> = bytes
            .strip_suffix(&[0])
            .unwrap()
            .split(|b| *b == 0)
            .collect();
        let mut expected = vec![dir.to_str().unwrap(), ""];
        expected.extend_from_slice(args);
        assert_eq!(
            fields,
            expected.iter().map(|s| s.as_bytes()).collect::<Vec<_>>(),
            "record {tag}: cwd, empty stdin, exact argv"
        );
    }

    fn no_records(&self) {
        assert_eq!(
            fs::read_dir(&self.records).unwrap().count(),
            0,
            "blocked commands must not run"
        );
    }
}

fn outcome(e: &Value, status: &str, reason: &str, code: &str, count: &str) {
    assert_eq!(e["qa_status"], status, "{e}");
    assert_eq!(e["qa_reason"], reason, "{e}");
    assert_eq!(e["qa_exit_code"], code, "{e}");
    assert_eq!(e["qa_suite_commands_count"], count, "{e}");
}

#[test]
fn quoted_arguments_reach_the_command_without_quote_characters() {
    let f = Fixture::new(None);
    let (e, log) = f.evidence(&[(
        "AUTODRIVE_QA_COMMAND",
        "qa-arg-recorder quoted --skip \"slow one\" 'single quoted' \"\" '$literal' \"a;b\"",
    )]);
    outcome(&e, "PASS", "", "0", "1");
    f.record(
        "quoted",
        &f.repo,
        &["--skip", "slow one", "single quoted", "", "$literal", "a;b"],
    );
    assert!(
        log.contains("QA-RECORDER-STDOUT") && log.contains("QA-RECORDER-STDERR"),
        "{log}"
    );
    assert!(
        e["qa_summary"]
            .as_str()
            .unwrap()
            .contains("QA-RECORDER-STDOUT")
    );
}

#[test]
fn singular_directory_handles_relative_absolute_spaces_dash_and_cdpath() {
    let f = Fixture::new(None);
    let decoy = f.root.join("cdpath");
    fs::create_dir_all(decoy.join("sub")).unwrap();
    for (n, dir) in [
        "sub".to_owned(),
        "suite with spaces".to_owned(),
        "-".to_owned(),
        f.root.join("absolute suite").to_str().unwrap().to_owned(),
    ]
    .iter()
    .enumerate()
    {
        let expected = f.repo.join(dir);
        fs::create_dir_all(&expected).unwrap();
        let tag = format!("dir-{n}");
        let command = format!("qa-arg-recorder {tag} \"one argument\"");
        let (e, _) = f.evidence(&[
            ("AUTODRIVE_QA_COMMAND", &command),
            ("AUTODRIVE_QA_DIR", dir),
            ("CDPATH", decoy.to_str().unwrap()),
        ]);
        outcome(&e, "PASS", "", "0", "1");
        assert_eq!(e["qa_dir"], dir.as_str());
        f.record(&tag, &expected, &["one argument"]);
    }
}

#[test]
fn unset_directory_defaults_to_repo_root_even_when_invoked_elsewhere() {
    let f = Fixture::new(None);
    // evidence() always starts inside repo, so explicitly start this invocation
    // elsewhere while REPO_PATH remains the selection authority.
    let command = "qa-arg-recorder default \"root argument\"";
    let out = f
        .child("bash")
        .current_dir(&f.home)
        .arg(workspace().join("amplifier-bundle/tools/autodrive_qa_evidence.sh"))
        .env("REPO_PATH", &f.repo)
        .env("AUTODRIVE_QA_BASE", "main")
        .env("AUTODRIVE_QA_COMMAND", command)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let e: Value = serde_json::from_slice(&out.stdout).unwrap();
    outcome(&e, "PASS", "", "0", "1");
    assert_eq!(e["qa_dir"], ".");
    f.record("default", &f.repo, &["root argument"]);
}

#[test]
fn quoted_executable_paths_and_compound_shell_commands_run() {
    let f = Fixture::new(None);
    let path = f.bin.join("recorder with spaces");
    let command = format!(
        "\"{}\" double \"two words\" && words=('two words') && '{}' single \"${{words[0]}}\"",
        path.display(),
        path.display()
    );
    let (e, _) = f.evidence(&[("AUTODRIVE_QA_COMMAND", &command)]);
    outcome(&e, "PASS", "", "0", "1");
    f.record("double", &f.repo, &["two words"]);
    f.record("single", &f.repo, &["two words"]);
}

#[test]
fn singular_and_plural_keep_separate_directories_order_and_first_failure() {
    let f = Fixture::new(None);
    let sub = f.repo.join("sub");
    fs::create_dir(&sub).unwrap();
    let (e, _) = f.evidence(&[
        ("AUTODRIVE_QA_DIR", "sub"),
        ("AUTODRIVE_QA_COMMAND", "qa-arg-recorder singular 'one argument'; touch ready; exit 23"),
        ("AUTODRIVE_QA_COMMANDS", "# ignored\n\n test -f sub/ready && qa-arg-recorder plural 'root argument'; exit 41\nqa-arg-recorder last"),
        // This file is created by the singular source before plural starts.
    ]);
    outcome(&e, "FAIL", "qa-command-failed", "23", "3");
    f.record("singular", &sub, &["one argument"]);
    f.record("plural", &f.repo, &["root argument"]);
    f.record("last", &f.repo, &[]);
}

#[test]
fn bash_determines_missing_executable_and_complete_command_status() {
    let f = Fixture::new(None);
    for (command, status, reason, code) in [
        (
            "issue-2441-command-that-does-not-exist",
            "FAIL",
            "qa-command-failed",
            "127",
        ),
        (
            "issue-2441-command-that-does-not-exist; true",
            "PASS",
            "",
            "0",
        ),
        ("exit 37", "FAIL", "qa-command-failed", "37"),
        ("true && exit 19", "FAIL", "qa-command-failed", "19"),
        ("true 'unterminated", "FAIL", "qa-command-failed", "2"),
        ("kill -TERM $$", "FAIL", "qa-command-failed", "143"),
    ] {
        let (e, _) = f.evidence(&[("AUTODRIVE_QA_COMMAND", command)]);
        outcome(&e, status, reason, code, "1");
    }
}

#[test]
fn invalid_directory_blocks_singular_execution_without_side_effects() {
    let f = Fixture::new(None);
    f.write("not-a-directory", "file");
    for dir in ["missing", "not-a-directory"] {
        let (e, _) = f.evidence(&[
            ("AUTODRIVE_QA_COMMAND", "qa-arg-recorder forbidden"),
            ("AUTODRIVE_QA_DIR", dir),
        ]);
        outcome(&e, "BLOCKED", "qa-command-missing", "", "0");
        assert!(e["qa_summary"].as_str().unwrap().contains(dir));
        f.no_records();
    }
}

#[test]
fn unsafe_committed_manifest_is_rejected_before_any_command_runs() {
    for source in [
        "command = \"qa-arg-recorder unsafe; touch sentinel\"\n",
        "command = \"qa-arg-recorder unsafe 'two words'\"\n",
        "command = \"qa-arg-recorder unsafe $(touch sentinel)\"\n",
    ] {
        let f = Fixture::new(Some(source));
        let (e, _) = f.evidence(&[]);
        outcome(&e, "BLOCKED", "qa-config-invalid", "", "0");
        assert!(
            e["qa_summary"].as_str().unwrap().contains("unsafe-command"),
            "{e}"
        );
        assert!(!f.repo.join("sentinel").exists());
        f.no_records();
    }
}

#[test]
fn env_precedence_and_set_but_empty_overrides_are_preserved() {
    let f = Fixture::new(Some(
        "command = \"qa-arg-recorder manifest; touch sentinel\"\n",
    ));
    let (e, _) = f.evidence(&[("AUTODRIVE_QA_COMMAND", "qa-arg-recorder env")]);
    outcome(&e, "PASS", "", "0", "1");
    assert_eq!(e["qa_command_source"], "env");
    f.record("env", &f.repo, &[]);
    assert!(!f.records.join("manifest").exists());
    assert!(!f.repo.join("sentinel").exists());
    for variable in ["AUTODRIVE_QA_COMMAND", "AUTODRIVE_QA_COMMANDS"] {
        let (e, _) = f.evidence(&[(variable, "")]);
        outcome(&e, "BLOCKED", "qa-command-missing", "", "0");
        assert_eq!(e["qa_command_source"], "env");
        assert_eq!(fs::read_dir(&f.records).unwrap().count(), 1);
    }
}

#[test]
fn safe_manifest_and_detected_commands_keep_restricted_execution() {
    let f = Fixture::new(Some(
        "command = \"qa-arg-recorder manifest plain\"\ndir = \"sub\"\n",
    ));
    fs::create_dir(f.repo.join("sub")).unwrap();
    let (e, _) = f.evidence(&[]);
    outcome(&e, "PASS", "", "0", "1");
    assert_eq!(e["qa_command_source"], "repo-file");
    f.record("manifest", &f.repo.join("sub"), &["plain"]);
    let f = Fixture::new(None);
    let (e, _) = f.evidence(&[]);
    outcome(&e, "PASS", "", "0", "1");
    assert_eq!(e["qa_command_source"], "detected");
    f.record(
        "test",
        &f.repo,
        &["--workspace", "--locked", "--no-fail-fast"],
    );
}

#[test]
fn plural_only_remains_at_repo_root_and_ignores_singular_directory() {
    let f = Fixture::new(None);
    let (e, _) = f.evidence(&[
        ("AUTODRIVE_QA_DIR", "does-not-exist"),
        (
            "AUTODRIVE_QA_COMMANDS",
            "qa-arg-recorder plural 'two words'",
        ),
    ]);
    outcome(&e, "PASS", "", "0", "1");
    assert_eq!(e["qa_dir"], ".");
    f.record("plural", &f.repo, &["two words"]);
}

#[test]
fn ci_source_keeps_safe_commands_and_rejects_shell_source() {
    for (run, source) in [
        ("cargo test plain", "ci"),
        ("cargo test; touch sentinel", "detected"),
    ] {
        let f = Fixture::new(None);
        f.write(".github/workflows/test.yml", &format!(
            "name: tests\non: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: {run}\n"
        ));
        f.commit();
        f.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        f.write(
            "src/main.rs",
            "fn main() { println!(\"another change\"); }\n",
        );
        f.commit();
        let (e, _) = f.evidence(&[]);
        outcome(&e, "PASS", "", "0", "1");
        assert_eq!(e["qa_command_source"], source, "{e}");
        let args: &[&str] = if source == "ci" {
            &["plain"]
        } else {
            &["--workspace", "--locked", "--no-fail-fast"]
        };
        f.record("test", &f.repo, args);
        assert!(!f.repo.join("sentinel").exists());
    }
}

#[test]
fn helper_preserves_arguments_directory_null_stdin_and_output() {
    let f = Fixture::new(None);
    let sub = f.repo.join("helper suite");
    fs::create_dir(&sub).unwrap();
    let input = f.root.join("input.txt");
    fs::write(&input, "caller input must not reach the QA command").unwrap();
    let out = f
        .child(env!("CARGO_BIN_EXE_amplihack"))
        .args(["orch", "helper", "autodrive-qa-command"])
        .arg(format!("--dir={}", sub.display()))
        .env(
            "AUTODRIVE_QA_COMMAND",
            "qa-arg-recorder helper 'two words' \"\"",
        )
        .stdin(fs::File::open(input).unwrap())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    f.record("helper", &sub, &["two words", ""]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("QA-RECORDER-STDOUT"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("QA-RECORDER-STDERR"));
}

#[test]
fn helper_propagates_exit_codes_and_unix_signal_status() {
    let f = Fixture::new(None);
    for (command, code) in [
        ("exit 0", 0),
        ("exit 23", 23),
        ("issue-2441-command-that-does-not-exist", 127),
        ("kill -TERM $$", 143),
    ] {
        let out = f.helper(Some(command), &f.repo);
        assert_eq!(out.status.code(), Some(code), "{command}: {out:?}");
    }
}

#[test]
fn helper_rejects_missing_input_and_unusable_directory() {
    let f = Fixture::new(None);
    for (command, dir) in [
        (None, f.repo.clone()),
        (Some("qa-arg-recorder forbidden"), f.repo.join("missing")),
    ] {
        let out = f.helper(command, &dir);
        assert!(!out.status.success(), "{out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.trim().is_empty(),
            "failure must diagnose the input: {out:?}"
        );
        // An unknown CLI subcommand is not an implementation of this contract.
        assert!(!stderr.contains("unrecognized subcommand"), "{stderr}");
        f.no_records();
    }
    // No Bash on PATH: the helper must diagnose spawn failure and return non-zero.
    let out = f
        .child(env!("CARGO_BIN_EXE_amplihack"))
        .args(["orch", "helper", "autodrive-qa-command"])
        .arg(format!("--dir={}", f.repo.display()))
        .env("PATH", &f.bin)
        .env("AUTODRIVE_QA_COMMAND", "qa-arg-recorder forbidden")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_ascii_lowercase().contains("bash"), "{stderr}");
    f.no_records();
}

#[test]
fn empty_directory_defaults_to_repository_root() {
    let f = Fixture::new(None);
    let (e, _) = f.evidence(&[
        ("AUTODRIVE_QA_COMMAND", "qa-arg-recorder empty-dir"),
        ("AUTODRIVE_QA_DIR", ""),
    ]);
    outcome(&e, "PASS", "", "0", "1");
    assert_eq!(e["qa_dir"], ".");
    f.record("empty-dir", &f.repo, &[]);
}

#[test]
fn missing_helper_fails_evidence_generation_without_running_commands() {
    let f = Fixture::new(None);
    fs::remove_file(f.bin.join("amplihack")).unwrap();
    let out = f
        .evidence_child()
        .env("AUTODRIVE_QA_COMMAND", "qa-arg-recorder forbidden")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "no fabricated evidence: {out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("QA command helper not found"));
    assert!(!f.root.join("evidence.json").exists());
    f.no_records();
    // Directory errors retain their existing evidence even without the helper.
    let (e, _) = f.evidence(&[
        ("AUTODRIVE_QA_COMMAND", "qa-arg-recorder forbidden"),
        ("AUTODRIVE_QA_DIR", "missing"),
    ]);
    outcome(&e, "BLOCKED", "qa-command-missing", "", "0");
    f.no_records();
}

#[test]
fn helper_is_resolved_before_changing_directories() {
    let f = Fixture::new(None);
    let sub = f.repo.join("sub");
    fs::create_dir(&sub).unwrap();
    let source = format!("'{}' relative-helper 'two words'", recorder().display());
    let out = f
        .evidence_child()
        .current_dir(&f.home)
        .env("PATH", "../bin:/usr/bin:/bin")
        .env("AUTODRIVE_QA_COMMAND", source)
        .env("AUTODRIVE_QA_DIR", "sub")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let e: Value = serde_json::from_slice(&out.stdout).unwrap();
    outcome(&e, "PASS", "", "0", "1");
    f.record("relative-helper", &sub, &["two words"]);
}

#[test]
fn established_binary_override_is_used() {
    let f = Fixture::new(None);
    fs::rename(f.bin.join("amplihack"), f.bin.join("custom helper")).unwrap();
    let (e, _) = f.evidence(&[
        (
            "AMPLIHACK_BIN",
            f.bin.join("custom helper").to_str().unwrap(),
        ),
        ("AUTODRIVE_QA_COMMAND", "qa-arg-recorder override"),
    ]);
    outcome(&e, "PASS", "", "0", "1");
    f.record("override", &f.repo, &[]);
}
