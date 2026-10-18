//! Issue #1570: `amplihack recipe run --context-file <PATH>` and
//! `--context-from-file KEY=PATH` deliver context of any size without putting
//! it in a single argv element.
//!
//! Runs the real `amplihack` binary. The recipe runner is replaced by a stub
//! (through `RECIPE_RUNNER_RS_PATH`) that records its argv and copies any
//! `--context-file` it is given, so the test sees exactly what amplihack hands
//! the runner: `--set KEY=VALUE` arguments, the runner's only context input.
//! A single pair longer than one argument is refused before the runner
//! starts. See [`pinned_runner`] for how the runner is found.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Linux `MAX_ARG_STRLEN`.
const MAX_ARG_STRLEN: usize = 131_072;

/// See `issue_1567_nested_profile.rs`: the binary lives in `bins/amplihack`.
fn amplihack_bin() -> PathBuf {
    static BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(p) = std::env::var_os("AMPLIHACK_TEST_BINARY") {
            return PathBuf::from(p);
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--quiet", "-p", "amplihack", "--bin", "amplihack"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("failed to run cargo build for the amplihack binary");
        assert!(status.success(), "cargo build -p amplihack failed");
        let bin = std::env::current_exe()
            .unwrap()
            .parent() // deps
            .and_then(Path::parent) // debug
            .unwrap()
            .join("amplihack");
        assert!(bin.is_file(), "no amplihack binary at {}", bin.display());
        bin
    })
    .clone()
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Records argv (NUL-separated) and a copy of `--context-file`, then reports
/// success in the runner's JSON format.
const STUB_RUNNER: &str = r#"#!/bin/sh
: > "$RECORD_DIR/argv.bin"
for a in "$@"; do printf '%s\0' "$a" >> "$RECORD_DIR/argv.bin"; done
prev=
for a in "$@"; do
  if [ "$prev" = "--context-file" ]; then cp "$a" "$RECORD_DIR/context.json"; fi
  prev=$a
done
printf '%s\n' '{"recipe_name":"issue-1570-context-file","success":true,"step_results":[],"context":{}}'
"#;

const RECIPE: &str = r#"name: issue-1570-context-file
description: Writes a context value to a file
version: 1.0.0
context: {}
steps:
  - id: write-value
    type: bash
    command: |
      printf '%s' "$RECIPE_VAR_big" > "$RECIPE_VAR_out_path"
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    records: PathBuf,
    tools: PathBuf,
    recipe: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let home = root.join("home");
        let records = root.join("records");
        let tools = root.join("tools");
        for d in [&home, &records, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        executable(&tools.join("recipe-runner-rs"), STUB_RUNNER);
        let recipe = root.join("recipe.yaml");
        fs::write(&recipe, RECIPE).unwrap();
        Self {
            _tmp: tmp,
            root,
            home,
            records,
            tools,
            recipe,
        }
    }

    fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    /// `amplihack recipe run <recipe> <args>` with no inherited environment
    /// except a minimal `PATH`, and `runner` as the recipe runner.
    fn run(&self, runner: &Path, extra_path: Option<&Path>, args: &[&str]) -> Output {
        let mut path = format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display());
        if let Some(extra) = extra_path {
            path = format!("{}:{path}", extra.display());
        }
        let mut c = Command::new(amplihack_bin());
        c.arg("recipe")
            .arg("run")
            .arg(&self.recipe)
            .args(args)
            .arg("-w")
            .arg(&self.root)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("RECORD_DIR", &self.records)
            .env("RECIPE_RUNNER_RS_PATH", runner)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
        c.output().expect("failed to run amplihack")
    }

    fn run_stub(&self, args: &[&str]) -> Output {
        self.run(&self.tools.join("recipe-runner-rs"), None, args)
    }

    fn runner_argv(&self) -> Vec<String> {
        let raw = fs::read(self.records.join("argv.bin")).expect("stub runner was not started");
        raw.split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8(s.to_vec()).unwrap())
            .collect()
    }

    /// The context the runner received, from `--set` arguments and from a
    /// `--context-file` copy, whichever amplihack used.
    fn runner_context(&self) -> BTreeMap<String, String> {
        let argv = self.runner_argv();
        let mut context = BTreeMap::new();
        for pair in argv.windows(2) {
            if (pair[0] == "--set" || pair[0] == "-c")
                && let Some((k, v)) = pair[1].split_once('=')
            {
                context.insert(k.to_string(), v.to_string());
            }
        }
        if let Ok(bytes) = fs::read(self.records.join("context.json")) {
            let json: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(&bytes).unwrap();
            for (k, v) in json {
                let v = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                context.insert(k, v);
            }
        }
        context
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn value_of_len(len: usize) -> String {
    let unit = "line é 'q' \"dq\" $(touch pwned) `x` \\ ;\n";
    let mut value = String::new();
    while value.len() < len {
        value.push_str(unit);
    }
    value
}

/// Two 100 KiB values (over 128 KiB together, each fitting in one argument)
/// reach the runner byte-exact as `--set` arguments, never `--context-file`,
/// which `recipe-runner-rs` rejects.
#[test]
fn context_file_and_two_100_kib_context_from_file_values_reach_the_runner_byte_exact() {
    let f = Fixture::new();
    let ctx = f.write(
        "ctx.json",
        r#"{"from_json":"yes","min_cycles":3,"big":"overridden by --context-from-file"}"#,
    );
    let value = value_of_len(100 * 1024);
    let big = f.write("big.txt", &value);
    let big2 = f.write("big2.txt", &value);
    let ctx_arg = ctx.display().to_string();
    let big_arg = format!("big={}", big.display());
    let big2_arg = format!("big2={}", big2.display());

    let out = f.run_stub(&[
        "--context-file",
        &ctx_arg,
        "--context-from-file",
        &big_arg,
        "--context-from-file",
        &big2_arg,
        "-c",
        "from_cli=1",
    ]);
    assert!(out.status.success(), "{}", describe(&out));

    let argv = f.runner_argv();
    assert!(
        !argv.iter().any(|a| a == "--context-file"),
        "recipe-runner-rs has no --context-file option"
    );
    for arg in &argv {
        assert!(
            arg.len() <= MAX_ARG_STRLEN,
            "runner argv element of {} bytes",
            arg.len()
        );
    }
    let context = f.runner_context();
    for key in ["big", "big2"] {
        assert!(
            context.get(key) == Some(&value),
            "{key} must arrive byte-exact (got {:?} bytes)",
            context.get(key).map(String::len)
        );
    }
    assert_eq!(context.get("from_json").map(String::as_str), Some("yes"));
    assert_eq!(context.get("min_cycles").map(String::as_str), Some("3"));
    assert_eq!(context.get("from_cli").map(String::as_str), Some("1"));
    assert!(!f.root.join("pwned").exists());
}

/// A single value longer than one argument cannot reach the runner, whose
/// only context input is `--set`; amplihack exits 1 naming the key and size
/// without starting the runner or printing the value.
#[test]
fn context_from_file_value_over_one_argument_exits_1_naming_the_key() {
    let f = Fixture::new();
    let value = value_of_len(200 * 1024);
    let big = f.write("big.txt", &value);
    let big_arg = format!("big={}", big.display());

    let out = f.run_stub(&["--context-from-file", &big_arg]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("recipe context value for key \"big\""),
        "{stderr}"
    );
    assert!(stderr.contains(&value.len().to_string()), "{stderr}");
    assert!(!stderr.contains("touch pwned"), "value must not be printed");
    assert!(
        !f.records.join("argv.bin").exists(),
        "the runner must not be started"
    );
}

#[test]
fn context_file_with_top_level_array_exits_1_with_fixed_message() {
    let f = Fixture::new();
    let ctx = f.write("ctx.json", r#"["SENTINEL_1570"]"#);
    let ctx_arg = ctx.display().to_string();
    let out = f.run_stub(&["--context-file", &ctx_arg]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "--context-file {ctx_arg}: top-level JSON value must be an object"
        )),
        "{}",
        describe(&out)
    );
    assert!(!stderr.contains("SENTINEL_1570"), "{stderr}");
    assert!(
        !f.records.join("argv.bin").exists(),
        "runner must not start"
    );
}

#[test]
fn context_from_file_without_equals_exits_1_with_fixed_message() {
    let f = Fixture::new();
    let out = f.run_stub(&["--context-from-file", "task_description"]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--context-from-file: expected KEY=PATH"),
        "{}",
        describe(&out)
    );
    assert!(
        !f.records.join("argv.bin").exists(),
        "runner must not start"
    );
}

#[test]
fn context_from_file_reserved_key_exits_1_with_fixed_message() {
    let f = Fixture::new();
    let v = f.write("v.txt", "SENTINEL_1570");
    let arg = format!("AMPLIHACK_HOME={}", v.display());
    let out = f.run_stub(&["--context-from-file", &arg]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--context-from-file: key 'AMPLIHACK_HOME' is reserved"),
        "{}",
        describe(&out)
    );
    assert!(!stderr.contains("SENTINEL_1570"), "{stderr}");
}

fn installed_runner() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("RECIPE_RUNNER_QA_BINARY") {
        return Some(PathBuf::from(explicit));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("recipe-runner-rs"))
        .find(|candidate| candidate.is_file())
}

/// Runner-backed: the bash step reads the value that `--context-from-file`
/// supplied. Small enough for the runner to export as `RECIPE_VAR_big`.
#[test]
fn runner_backed_step_receives_context_from_file_value() {
    let required = std::env::var_os("RECIPE_RUNNER_QA_BINARY").is_some()
        || std::env::var_os("AMPLIHACK_1570_REQUIRE_RECIPE_RUNNER").is_some();
    let runner = match installed_runner() {
        Some(r) if Command::new(&r).arg("--version").output().is_ok() => r,
        _ if required => panic!("recipe-runner-rs is required for the runner-backed scenario"),
        _ => {
            eprintln!("recipe-runner-rs unavailable; skipping runner-backed scenario");
            return;
        }
    };
    let f = Fixture::new();
    let out_path = f.root.join("out.txt");
    let ctx = f.write(
        "ctx.json",
        serde_json::to_vec(&serde_json::json!({ "out_path": out_path })).unwrap(),
    );
    let value = value_of_len(16 * 1024);
    let big = f.write("big.txt", &value);
    let ctx_arg = ctx.display().to_string();
    let big_arg = format!("big={}", big.display());

    let out = f.run(
        &runner,
        runner.parent(),
        &["--context-file", &ctx_arg, "--context-from-file", &big_arg],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let written = fs::read_to_string(&out_path).expect("bash step must write the value");
    assert!(written == value, "value must arrive byte-exact");
    assert!(!f.root.join("pwned").exists());
}

/// Runner-backed: a context totalling over 128 KiB (two 100 KiB values)
/// starts the real runner, and a bash step receives both values.
#[test]
fn runner_backed_context_over_128_kib_in_total_reaches_the_step() {
    let required = std::env::var_os("RECIPE_RUNNER_QA_BINARY").is_some()
        || std::env::var_os("AMPLIHACK_1570_REQUIRE_RECIPE_RUNNER").is_some();
    let runner = match installed_runner() {
        Some(r) if Command::new(&r).arg("--version").output().is_ok() => r,
        _ if required => panic!("recipe-runner-rs is required for the runner-backed scenario"),
        _ => {
            eprintln!("recipe-runner-rs unavailable; skipping runner-backed scenario");
            return;
        }
    };
    let f = Fixture::new();
    fs::write(
        &f.recipe,
        r#"name: issue-1570-context-total
description: Two large context values
version: 1.0.0
context:
  a: ""
  b: ""
  out_dir: ""
steps:
  - id: write
    type: bash
    command: |
      printf '%s' "$RECIPE_VAR_a" > "$RECIPE_VAR_out_dir/a.txt"
      printf '%s' "$RECIPE_VAR_b" > "$RECIPE_VAR_out_dir/b.txt"
"#,
    )
    .unwrap();
    let out_dir = f.root.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let value = value_of_len(100 * 1024);
    let a = f.write("a.txt", &value);
    let b = f.write("b.txt", &value);
    let a_arg = format!("a={}", a.display());
    let b_arg = format!("b={}", b.display());
    let out_arg = format!("out_dir={}", out_dir.display());

    let out = f.run(
        &runner,
        runner.parent(),
        &[
            "--context-from-file",
            &a_arg,
            "--context-from-file",
            &b_arg,
            "-c",
            &out_arg,
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    for name in ["a.txt", "b.txt"] {
        let got = fs::read_to_string(out_dir.join(name)).unwrap();
        assert!(got == value, "{name} must arrive byte-exact");
    }
    assert!(!f.root.join("pwned").exists());
}

/// Step `make` prints the 200 KiB source file, so `payload` exists only
/// inside the run; step `read` reads it both ways and writes each copy out.
const LARGE_VALUE_RECIPE: &str = r#"name: issue-1570-runner-large-value
description: Reads a spilled context value in a bash step
version: 1.0.0
context:
  source: ""
  out_dir: ""
  helper_dir: ""
steps:
  - id: make
    type: bash
    command: |
      cat -- "$RECIPE_VAR_source"
    output: payload
  - id: read
    type: bash
    command: |
      . "$RECIPE_VAR_helper_dir/recipe_context_value.sh" || exit 1
      printf '%s' "$RECIPE_VAR_payload" > "$RECIPE_VAR_out_dir/var.txt"
      write_context_value payload "$RECIPE_VAR_out_dir/helper.txt"
"#;

/// The runner the large-value test must use: `RECIPE_RUNNER_RS_PATH` (set by
/// CI to the runner installed at `claude-plugin/recipe-runner.rev`), then
/// `RECIPE_RUNNER_QA_BINARY`, then `recipe-runner-rs` on `PATH`. A missing
/// runner fails the test; it never passes without running.
fn pinned_runner() -> (PathBuf, bool) {
    if let Some(explicit) = std::env::var_os("RECIPE_RUNNER_RS_PATH") {
        return (PathBuf::from(explicit), true);
    }
    let runner = installed_runner().expect(
        "recipe-runner-rs is required: install it at claude-plugin/recipe-runner.rev \
         (claude-plugin/bin/install-runtime) or set RECIPE_RUNNER_RS_PATH",
    );
    (runner, false)
}

/// Whether `runner` warns about unknown step fields, which runners older than
/// the pin do not. Those runners also cannot spill a large value.
fn runner_reports_unknown_fields(runner: &Path, dir: &Path) -> bool {
    let probe = dir.join("probe.yaml");
    fs::write(
        &probe,
        "name: probe\ndescription: probe\nversion: 1.0.0\ncontext: {}\nsteps:\n  - id: s\n    type: bash\n    probe_field_1570: 1\n    command: \"true\"\n",
    )
    .unwrap();
    let out = Command::new(runner)
        .arg(&probe)
        .arg("--validate-only")
        .output()
        .expect("failed to run recipe-runner-rs");
    String::from_utf8_lossy(&out.stdout).contains("probe_field_1570")
        || String::from_utf8_lossy(&out.stderr).contains("probe_field_1570")
}

/// D5: the pinned runner delivers a value over 128 KiB to a bash step, both
/// as `$RECIPE_VAR_payload` and through `write_context_value`.
#[test]
fn recipe_runner_delivers_200k_value_to_bash_step() {
    let (runner, explicit) = pinned_runner();
    let f = Fixture::new();
    if !runner_reports_unknown_fields(&runner, &f.root) {
        assert!(
            !explicit,
            "RECIPE_RUNNER_RS_PATH={} is older than claude-plugin/recipe-runner.rev",
            runner.display()
        );
        eprintln!(
            "skipping: {} is older than claude-plugin/recipe-runner.rev",
            runner.display()
        );
        return;
    }
    fs::write(&f.recipe, LARGE_VALUE_RECIPE).unwrap();
    let out_dir = f.root.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let helper_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools")
        .canonicalize()
        .unwrap();
    // No trailing newline, so the runner's handling of step output whitespace
    // cannot change the bytes.
    let value = value_of_len(200 * 1024) + "end";
    assert!(value.len() > MAX_ARG_STRLEN);
    let source = f.write("payload.txt", &value);

    let out = Command::new(&runner)
        .arg(&f.recipe)
        .arg("-C")
        .arg(&f.root)
        .arg("--set")
        .arg(format!("source={}", source.display()))
        .arg("--set")
        .arg(format!("out_dir={}", out_dir.display()))
        .arg("--set")
        .arg(format!("helper_dir={}", helper_dir.display()))
        .env("HOME", &f.home)
        .env_remove("AMPLIHACK_CONTEXT_FILE")
        .output()
        .expect("failed to run recipe-runner-rs");
    assert!(out.status.success(), "{}", describe(&out));
    for name in ["var.txt", "helper.txt"] {
        let got = fs::read(out_dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            got == value.as_bytes(),
            "{name}: {} bytes, expected {} byte for byte",
            got.len(),
            value.len()
        );
    }
    assert!(!f.root.join("pwned").exists());
}
