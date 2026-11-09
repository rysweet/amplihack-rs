//! Issue #2190 — recipe check steps return the real exit status of tests and
//! hooks, and keep their stderr in the step log.
//!
//! `consensus-test-fix.yaml` `step7-run-tests` used to run
//! `cargo test ... 2>/dev/null || echo "Tests need verification"` and then
//! print `{"tests_run": true}` whatever happened, so a failing test suite
//! looked the same as a passing one. `consensus-merge-finalize.yaml`
//! `step14-check-ci` did the same with `gh pr checks`.
//!
//! These tests run the real step commands, taken from the YAML, in `bash`
//! with `env_clear()`, a temporary `HOME` and `TMPDIR`, and a `PATH` built
//! only from stub tools plus links to the system utilities the steps need.
//! The real `cargo`, `npm`, `pre-commit` and `gh` are never reachable.
//! No test sleeps or waits on a clock: each stub exits at once with a fixed
//! status.
//!
//! Contract (see `docs/reference/recipe-check-exit-status.md`):
//! - stdout is exactly one JSON object whose values are all strings;
//! - an exit code of `""` means the check did not run because the project
//!   does not ask for it; any check that runs and exits non-zero (127 and
//!   gh's 8 included) makes the step report not passing;
//! - the check's own output reaches the step's stderr and the `log_file`;
//! - the step itself exits 0, and the routing conditions send failures to
//!   the fix / diagnose agents.
//!
//! The guard `no_recipe_masks_check_exit_status` scans every recipe for a
//! check command followed by `2>/dev/null`, `&>/dev/null`, `|| echo`,
//! `|| true` or `|| :`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TEST_FIX: &str = "consensus-test-fix.yaml";
const MERGE_FINALIZE: &str = "consensus-merge-finalize.yaml";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn recipes_dir() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes")
}

fn recipe(name: &str) -> serde_yaml::Value {
    let path = recipes_dir().join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{name} must parse: {e}"))
}

fn step(recipe_name: &str, step_id: &str) -> serde_yaml::Value {
    recipe(recipe_name)["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .unwrap_or_else(|| panic!("{recipe_name}: no step {step_id}"))
        .clone()
}

fn step_command(recipe_name: &str, step_id: &str) -> String {
    step(recipe_name, step_id)["command"]
        .as_str()
        .unwrap_or_else(|| panic!("{recipe_name}: step {step_id} has no command"))
        .to_string()
}

fn step_str(recipe_name: &str, step_id: &str, key: &str) -> String {
    step(recipe_name, step_id)[key]
        .as_str()
        .unwrap_or_else(|| panic!("{recipe_name}: step {step_id} has no {key}"))
        .to_string()
}

fn executable(path: &Path, text: &str) {
    crate::support::exec_stub::write_executable(path, text, 0o755);
}

/// System utilities a step may use. Each one found on the host `PATH` is
/// linked into `sys/`; `cargo`, `npm`, `pre-commit` and `gh` are never
/// linked, so only the stubs in `bin/` can answer for them.
const SYSTEM_TOOLS: &[&str] = &[
    "bash", "sh", "env", "cat", "tail", "head", "mktemp", "rm", "tr", "wc", "sed", "grep",
    "dirname", "basename", "chmod", "ls", "stat", "touch", "mkdir", "cp", "mv", "printf", "echo",
    "true", "false", "test", "[", "yes", "git", "jq", "id", "date", "awk", "cut", "tee",
    "readlink", "realpath", "od",
];

const NEVER_LINKED: &[&str] = &["cargo", "npm", "pre-commit", "gh", "pytest"];

fn find_on_host_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

struct Sandbox {
    tmp: tempfile::TempDir,
    bin: PathBuf,
    sys: PathBuf,
    work: PathBuf,
    private_tmp: PathBuf,
    calls: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let sys = tmp.path().join("sys");
        let work = tmp.path().join("work");
        let private_tmp = tmp.path().join("tmp");
        let calls = tmp.path().join("calls");
        for d in [&bin, &sys, &work, &private_tmp, &calls] {
            std::fs::create_dir_all(d).unwrap();
        }
        for tool in SYSTEM_TOOLS {
            if NEVER_LINKED.contains(tool) {
                continue;
            }
            if let Some(real) = find_on_host_path(tool) {
                std::os::unix::fs::symlink(&real, sys.join(tool)).unwrap();
            }
        }
        Sandbox {
            tmp,
            bin,
            sys,
            work,
            private_tmp,
            calls,
        }
    }

    fn path_var(&self) -> String {
        format!("{}:{}", self.bin.display(), self.sys.display())
    }

    /// A stub that records its argv in `calls/<name>`, prints `stdout` and
    /// `stderr` lines, and exits with `code`.
    fn stub(&self, name: &str, code: i32, stdout: &str, stderr: &str) {
        let body = format!(
            "#!/usr/bin/env bash\n\
             printf '%s\\n' \"$*\" >> \"$STUB_CALLS/{name}\"\n\
             printf '%s\\n' '{stdout}'\n\
             printf '%s\\n' '{stderr}' >&2\n\
             exit {code}\n"
        );
        executable(&self.bin.join(name), &body);
    }

    /// A stub with a caller-written body after the call-recording line.
    fn stub_body(&self, name: &str, body: &str) {
        let text = format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"$STUB_CALLS/{name}\"\n{body}\n"
        );
        executable(&self.bin.join(name), &text);
    }

    fn called(&self, name: &str) -> bool {
        self.calls.join(name).is_file()
    }

    fn write(&self, rel: &str, text: &str) {
        std::fs::write(self.work.join(rel), text).unwrap();
    }

    fn run(&self, recipe_name: &str, step_id: &str) -> Output {
        Command::new(self.sys.join("bash"))
            .arg("-c")
            .arg(step_command(recipe_name, step_id))
            .env_clear()
            .env("PATH", self.path_var())
            .env("HOME", self.tmp.path())
            .env("TMPDIR", &self.private_tmp)
            .env("STUB_CALLS", &self.calls)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .current_dir(&self.work)
            .output()
            .expect("spawn bash")
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new(self.sys.join("git"))
            .args(args)
            .env_clear()
            .env("PATH", self.path_var())
            .env("HOME", self.tmp.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .current_dir(&self.work)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// stdout must be exactly one JSON object whose values are all strings.
fn json_object(out: &Output) -> serde_json::Map<String, serde_json::Value> {
    let text = stdout(out);
    let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        let head: String = text.chars().take(600).collect();
        panic!(
            "stdout must be exactly one JSON object ({e}); stdout starts:\n{head}\nstderr:\n{}",
            stderr(out).chars().take(2000).collect::<String>()
        )
    });
    let obj = value
        .as_object()
        .unwrap_or_else(|| panic!("stdout JSON must be an object, got {value}"))
        .clone();
    for (k, v) in &obj {
        assert!(
            v.is_string(),
            "field {k} must be a JSON string (\"true\"/\"false\"/\"0\"/\"\"), got {v}"
        );
    }
    obj
}

fn field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    obj.get(key)
        .unwrap_or_else(|| panic!("missing field {key} in {obj:?}"))
        .as_str()
        .unwrap()
        .to_string()
}

fn assert_step_exit_zero(out: &Output) {
    assert!(
        out.status.success(),
        "the step must exit 0 after printing its JSON (routing is the failure path); \
         status {:?}\nstderr:\n{}",
        out.status,
        stderr(out)
    );
}

const TEST_FIX_FIELDS: &[&str] = &[
    "tests_run",
    "test_command",
    "test_exit_code",
    "precommit_command",
    "precommit_exit_code",
    "all_passing",
    "log_file",
];

fn assert_test_fix_schema(obj: &serde_json::Map<String, serde_json::Value>) {
    for f in TEST_FIX_FIELDS {
        assert!(
            obj.contains_key(*f),
            "step7-run-tests JSON lacks {f}: {obj:?}"
        );
    }
}

/// A Cargo project with a pre-commit config; cargo and pre-commit stubs
/// exit with the given codes.
fn cargo_project(cargo_rc: i32, precommit_rc: i32) -> Sandbox {
    let sb = Sandbox::new();
    sb.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    );
    sb.write(".pre-commit-config.yaml", "repos: []\n");
    sb.stub(
        "cargo",
        cargo_rc,
        "STUB_CARGO_STDOUT",
        if cargo_rc == 0 {
            "STUB_CARGO_OK"
        } else {
            "STUB_CARGO_FAIL"
        },
    );
    sb.stub(
        "pre-commit",
        precommit_rc,
        "STUB_PRECOMMIT_STDOUT",
        if precommit_rc == 0 {
            "STUB_PRECOMMIT_OK"
        } else {
            "STUB_PRECOMMIT_FAIL"
        },
    );
    sb
}

// ---------------------------------------------------------------------------
// consensus-test-fix.yaml: step7-run-tests
// ---------------------------------------------------------------------------

#[test]
fn a_failing_cargo_test_makes_the_step_report_not_passing() {
    let sb = cargo_project(1, 0);
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_test_fix_schema(&obj);
    assert_eq!(field(&obj, "test_exit_code"), "1");
    assert_eq!(field(&obj, "all_passing"), "false");
    assert_eq!(field(&obj, "precommit_exit_code"), "0");
    assert!(
        field(&obj, "test_command").contains("cargo"),
        "test_command must name the cargo command: {obj:?}"
    );
    assert!(
        sb.called("cargo"),
        "the cargo test command must actually run"
    );
    assert!(
        stderr(&out).contains("STUB_CARGO_FAIL"),
        "cargo's stderr must reach the step log, not /dev/null; stderr:\n{}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("STUB_CARGO"),
        "check output must not leak onto stdout (stdout holds only JSON)"
    );
}

#[test]
fn cargo_exit_101_is_reported_verbatim() {
    let sb = cargo_project(101, 0);
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "test_exit_code"), "101");
    assert_eq!(field(&obj, "all_passing"), "false");
}

#[test]
fn every_check_passing_reports_all_passing_true() {
    let sb = cargo_project(0, 0);
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_test_fix_schema(&obj);
    assert_eq!(field(&obj, "test_exit_code"), "0");
    assert_eq!(field(&obj, "precommit_exit_code"), "0");
    assert_eq!(field(&obj, "all_passing"), "true");
    assert!(sb.called("cargo") && sb.called("pre-commit"));
}

#[test]
fn a_failing_pre_commit_makes_the_step_report_not_passing() {
    let sb = cargo_project(0, 1);
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "test_exit_code"), "0");
    assert_eq!(field(&obj, "precommit_exit_code"), "1");
    assert_eq!(field(&obj, "all_passing"), "false");
    assert!(
        stderr(&out).contains("STUB_PRECOMMIT_FAIL"),
        "pre-commit's stderr must reach the step log; stderr:\n{}",
        stderr(&out)
    );
}

#[test]
fn pre_commit_runs_with_all_files() {
    let sb = cargo_project(0, 0);
    let _ = sb.run(TEST_FIX, "step7-run-tests");
    let argv = std::fs::read_to_string(sb.calls.join("pre-commit")).unwrap_or_default();
    assert!(
        argv.contains("run") && argv.contains("--all-files"),
        "pre-commit must be invoked as `pre-commit run --all-files`, got: {argv:?}"
    );
}

/// Case c2: the project asks for pre-commit but it is not installed. The step
/// must not look on PATH first and skip; it runs, gets 127 and reports it.
#[test]
fn configured_pre_commit_missing_from_path_reports_127() {
    let sb = Sandbox::new();
    sb.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    );
    sb.write(".pre-commit-config.yaml", "repos: []\n");
    sb.stub("cargo", 0, "ok", "ok");
    assert!(
        !sb.bin.join("pre-commit").exists() && !sb.sys.join("pre-commit").exists(),
        "fixture must have no pre-commit on PATH"
    );
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "precommit_exit_code"), "127");
    assert_eq!(field(&obj, "all_passing"), "false");
}

/// No `.pre-commit-config.yaml`: the project does not ask for pre-commit, so
/// it does not run and that is not a failure.
#[test]
fn no_pre_commit_config_is_skipped_not_failed() {
    let sb = Sandbox::new();
    sb.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    );
    sb.stub("cargo", 0, "ok", "ok");
    sb.stub("pre-commit", 1, "SHOULD_NOT_RUN", "SHOULD_NOT_RUN");
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "precommit_command"), "none");
    assert_eq!(field(&obj, "precommit_exit_code"), "");
    assert_eq!(field(&obj, "all_passing"), "true");
    assert!(
        !sb.called("pre-commit"),
        "pre-commit must not run without a config"
    );
    assert!(
        stderr(&out).contains("[skip]"),
        "a skipped check must say so on stderr; stderr:\n{}",
        stderr(&out)
    );
}

/// A missing cargo is not a skip: the project has a Cargo.toml, so a 127 is
/// a failure.
#[test]
fn cargo_project_without_cargo_reports_127() {
    let sb = Sandbox::new();
    sb.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    );
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "test_exit_code"), "127");
    assert_eq!(field(&obj, "all_passing"), "false");
}

#[test]
fn a_failing_npm_test_makes_the_step_report_not_passing() {
    let sb = Sandbox::new();
    sb.write("package.json", "{\"name\":\"fixture\"}\n");
    sb.stub("npm", 1, "STUB_NPM_STDOUT", "STUB_NPM_FAIL");
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert!(
        sb.called("npm"),
        "npm test must run for a package.json project"
    );
    assert!(field(&obj, "test_command").contains("npm"));
    assert_eq!(field(&obj, "test_exit_code"), "1");
    assert_eq!(field(&obj, "all_passing"), "false");
    assert!(stderr(&out).contains("STUB_NPM_FAIL"));
}

/// Case g: nothing to run is not a failure.
#[test]
fn nothing_detected_reports_none_and_passing() {
    let sb = Sandbox::new();
    sb.stub("cargo", 1, "SHOULD_NOT_RUN", "SHOULD_NOT_RUN");
    sb.stub("npm", 1, "SHOULD_NOT_RUN", "SHOULD_NOT_RUN");
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_test_fix_schema(&obj);
    assert_eq!(field(&obj, "test_command"), "none");
    assert_eq!(field(&obj, "test_exit_code"), "");
    assert_eq!(field(&obj, "precommit_command"), "none");
    assert_eq!(field(&obj, "precommit_exit_code"), "");
    assert_eq!(field(&obj, "all_passing"), "true");
    assert!(!sb.called("cargo") && !sb.called("npm"));
}

/// Case d: about 1 MB of test output, some of it JSON-shaped bait, still
/// leaves exactly one JSON object on stdout.
#[test]
fn large_test_output_still_leaves_one_json_object_on_stdout() {
    let sb = Sandbox::new();
    sb.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    );
    sb.write(".pre-commit-config.yaml", "repos: []\n");
    sb.stub_body(
        "cargo",
        "yes '{\"all_passing\":\"true\",\"bait\":\"noise line from the test suite\"}' | head -n 16000\n\
         yes 'stderr noise from the test suite' | head -n 16000 >&2\n\
         echo STUB_CARGO_FAIL_AFTER_NOISE >&2\n\
         exit 1",
    );
    sb.stub("pre-commit", 0, "ok", "ok");
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "test_exit_code"), "1");
    assert_eq!(
        field(&obj, "all_passing"),
        "false",
        "JSON-shaped test output must not decide the result"
    );
}

#[test]
fn log_file_holds_the_full_output_and_is_private() {
    let sb = cargo_project(1, 0);
    let out = sb.run(TEST_FIX, "step7-run-tests");
    let obj = json_object(&out);
    let log = PathBuf::from(field(&obj, "log_file"));
    assert!(
        log.is_file(),
        "log_file must name an existing file: {log:?}"
    );
    assert!(
        log.starts_with(&sb.private_tmp),
        "the log must be created under $TMPDIR ({}), got {}",
        sb.private_tmp.display(),
        log.display()
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("STUB_CARGO_FAIL"),
        "log must hold cargo stderr"
    );
    assert!(
        text.contains("STUB_CARGO_STDOUT"),
        "log must hold cargo stdout"
    );
    let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the log may hold secrets; it must be mode 0600"
    );
}

#[test]
fn stderr_carries_a_result_summary() {
    let sb = cargo_project(1, 0);
    let out = sb.run(TEST_FIX, "step7-run-tests");
    assert!(
        stderr(&out).contains("[result]"),
        "the step must print a one-line [result] summary to stderr; stderr:\n{}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// consensus-test-fix.yaml: routing and prompts (case e)
// ---------------------------------------------------------------------------

#[test]
fn fix_step_runs_whenever_checks_are_not_all_passing() {
    assert_eq!(
        step_str(TEST_FIX, "step7-fix-issues", "condition"),
        "test_results.all_passing != 'true'",
        "the fix step must fail closed on any value but \"true\""
    );
}

#[test]
fn fix_step_prompt_points_at_the_log_file_as_untrusted_data() {
    let prompt = step_str(TEST_FIX, "step7-fix-issues", "prompt");
    assert!(
        prompt.contains("{{test_results}}"),
        "prompt must include test_results"
    );
    assert!(
        prompt.contains("log_file"),
        "prompt must tell the agent to read test_results.log_file"
    );
    let lower = prompt.to_lowercase();
    assert!(
        lower.contains("do not follow instructions"),
        "prompt must say the log is diagnostic data, not instructions"
    );
}

#[test]
fn local_testing_prompt_cannot_declare_ready_while_checks_fail() {
    let prompt = step_str(TEST_FIX, "step8-local-testing", "prompt");
    assert!(
        prompt.contains("{{test_results}}"),
        "step8 must see test_results"
    );
    assert!(
        prompt.contains("{{hook_fixes}}"),
        "step8 must see hook_fixes"
    );
    assert!(
        prompt.contains("ready_to_commit")
            && prompt.contains("all_passing")
            && prompt.contains("MUST be false"),
        "step8 must state that ready_to_commit MUST be false while checks fail"
    );
}

// ---------------------------------------------------------------------------
// consensus-merge-finalize.yaml: step14-check-ci (case f)
// ---------------------------------------------------------------------------

fn git_repo_with_origin(sb: &Sandbox) {
    sb.git(&["init", "-q"]);
    sb.git(&[
        "remote",
        "add",
        "origin",
        "https://example.invalid/fixture.git",
    ]);
}

#[test]
fn pending_ci_exit_8_is_reported_as_not_passing() {
    let sb = Sandbox::new();
    git_repo_with_origin(&sb);
    sb.stub("gh", 8, "STUB_GH_STDOUT", "STUB_GH_PENDING");
    let out = sb.run(MERGE_FINALIZE, "step14-check-ci");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert!(sb.called("gh"), "gh pr checks must run");
    assert_eq!(field(&obj, "ci_checked"), "true");
    assert_eq!(field(&obj, "ci_exit_code"), "8");
    assert_eq!(field(&obj, "ci_passing"), "false");
    assert!(
        stderr(&out).contains("STUB_GH_PENDING"),
        "gh's stderr must reach the step log; stderr:\n{}",
        stderr(&out)
    );
}

#[test]
fn failing_ci_exit_1_is_reported_as_not_passing() {
    let sb = Sandbox::new();
    git_repo_with_origin(&sb);
    sb.stub("gh", 1, "STUB_GH_STDOUT", "STUB_GH_FAIL");
    let out = sb.run(MERGE_FINALIZE, "step14-check-ci");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "ci_exit_code"), "1");
    assert_eq!(field(&obj, "ci_passing"), "false");
}

#[test]
fn passing_ci_is_reported_as_passing() {
    let sb = Sandbox::new();
    git_repo_with_origin(&sb);
    sb.stub("gh", 0, "STUB_GH_STDOUT", "STUB_GH_OK");
    let out = sb.run(MERGE_FINALIZE, "step14-check-ci");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "ci_checked"), "true");
    assert_eq!(field(&obj, "ci_exit_code"), "0");
    assert_eq!(field(&obj, "ci_passing"), "true");
    assert!(
        !stdout(&out).contains("STUB_GH_STDOUT"),
        "gh output must go to stderr; stdout holds only JSON"
    );
}

#[test]
fn no_git_repo_skips_ci_without_running_gh() {
    let sb = Sandbox::new();
    sb.stub("gh", 99, "GH_MUST_NOT_RUN", "GH_MUST_NOT_RUN");
    let out = sb.run(MERGE_FINALIZE, "step14-check-ci");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "ci_checked"), "false");
    assert_eq!(field(&obj, "ci_exit_code"), "");
    assert_eq!(field(&obj, "ci_passing"), "");
    assert!(!sb.called("gh"));
    assert!(stderr(&out).contains("[skip]"));
}

#[test]
fn git_repo_without_origin_skips_ci_without_running_gh() {
    let sb = Sandbox::new();
    sb.git(&["init", "-q"]);
    sb.stub("gh", 99, "GH_MUST_NOT_RUN", "GH_MUST_NOT_RUN");
    let out = sb.run(MERGE_FINALIZE, "step14-check-ci");
    assert_step_exit_zero(&out);
    let obj = json_object(&out);
    assert_eq!(field(&obj, "ci_checked"), "false");
    assert_eq!(field(&obj, "ci_exit_code"), "");
    assert_eq!(field(&obj, "ci_passing"), "");
    assert!(!sb.called("gh"));
}

#[test]
fn diagnose_step_runs_only_for_checked_and_not_passing_ci() {
    assert_eq!(
        step_str(MERGE_FINALIZE, "step14-diagnose-ci", "condition"),
        "ci_status.ci_checked == 'true' and ci_status.ci_passing != 'true'"
    );
}

// ---------------------------------------------------------------------------
// Guard: no recipe masks a check command's exit status or stderr
// ---------------------------------------------------------------------------

const CHECK_COMMANDS: &[&str] = &[
    "cargo test",
    "cargo clippy",
    "cargo fmt --check",
    "npm test",
    "pytest",
    "pre-commit run",
    "gh pr checks",
];

const MASKS: &[&str] = &["2>/dev/null", "&>/dev/null", "|| echo", "|| true", "|| :"];

/// The part of `s` before the first single `|` (a pipe, not `||`).
fn before_first_pipe(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'|' {
            if i + 1 < b.len() && b[i + 1] == b'|' {
                i += 2;
                continue;
            }
            return &s[..i];
        }
        i += 1;
    }
    s
}

/// Returns the mask found after a check command on this line, if any.
fn masking_in_line(line: &str) -> Option<&'static str> {
    if line.trim_start().starts_with('#') {
        return None;
    }
    for cmd in CHECK_COMMANDS {
        if let Some(pos) = line.find(cmd) {
            let rest = before_first_pipe(&line[pos + cmd.len()..]);
            if let Some(m) = MASKS.iter().find(|m| rest.contains(**m)) {
                return Some(m);
            }
        }
    }
    None
}

#[test]
fn masking_matcher_flags_the_original_lines() {
    let original = [
        r#"        cargo test --workspace --locked 2>/dev/null || echo "Tests need verification""#,
        r#"        npm test 2>/dev/null || echo "Tests need verification""#,
        r#"      pre-commit run --all-files 2>/dev/null || echo "Pre-commit checks need attention""#,
        r#"          gh pr checks 2>/dev/null || echo "CI check pending""#,
        r#"        JSON="$(gh pr checks "$PR" --json name,state,bucket 2>/dev/null)""#,
        "cargo clippy --all-targets || true",
        "cargo fmt --check || :",
        "pytest -q &>/dev/null",
    ];
    for line in original {
        assert!(masking_in_line(line).is_some(), "must flag: {line}");
    }
}

#[test]
fn masking_matcher_allows_the_honest_forms() {
    let honest = [
        r#"        cargo test --workspace --locked >>"$LOG" 2>&1; TEST_RC=$?"#,
        r#"      pre-commit run --all-files >>"$LOG" 2>&1; PC_RC=$?"#,
        r#"          if gh pr checks >&2; then CI_RC=0; else CI_RC=$?; fi"#,
        r#"        JSON="$(gh pr checks "$PR" --json name,state,bucket)""#,
        r#"      gh pr checks "$PR" --json name | jq -r '.[]' 2>/dev/null"#,
        r#"      # old: cargo test 2>/dev/null || echo "masked""#,
        r#"      cd "${REPO_PATH:-.}" 2>/dev/null || true"#,
    ];
    for line in honest {
        assert_eq!(masking_in_line(line), None, "must allow: {line}");
    }
}

#[test]
fn no_recipe_masks_check_exit_status() {
    let mut offenders = Vec::new();
    let dir = recipes_dir();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("reading {}: {err}", dir.display()))
        .map(|e| {
            e.unwrap_or_else(|err| panic!("reading {}: {err}", dir.display()))
                .path()
        })
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    entries.sort();
    assert!(
        !entries.is_empty(),
        "no recipes found under {:?}",
        recipes_dir()
    );
    for path in entries {
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            if let Some(mask) = masking_in_line(line) {
                offenders.push(format!(
                    "{}:{}: `{mask}` hides a check's exit status or stderr: {}",
                    path.file_name().unwrap().to_string_lossy(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "issue #2190: check commands must return their real exit status and keep \
         stderr in the step log:\n{}",
        offenders.join("\n")
    );
}
