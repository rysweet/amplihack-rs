//! #3053, #2996 and #2997: every step of auto-drive-to-merge and
//! auto-drive-light reads and writes the one state folder the runner bound.
//! A folder an agent step reports (here A, from the build step and from the
//! preflights) is information only and never decides where records go.
//!
//! What is real: the CLI, the pinned recipe runner, the drive recipes, the
//! crusty loop, the merge loop, finalisation, the trusted merge tool and the
//! authorisation helper. What is a stand-in: `gh`, the agent steps (bash steps
//! in a copy of the bundle, which report folder A), the build recipe (reports
//! folder A) and `autodrive_loop.sh` (a fake driver that writes the review's
//! records into the folder it is given). A `codex` stand-in fails if any agent
//! is reached. Nothing touches the real home, GitHub or an agent.
//!
//! Each test fails on main for the reason in its comment and passes once every
//! hand-off and bash reader uses the runner-bound folder B.

#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use serde_yaml::Value as Yaml;

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;

const PR: &str = "3053";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The skills a drive's launch check requires (`REQUIRED_ASSETS` in
/// `orch/autodrive/trusted.rs`). No other skill is read once the agent steps
/// are stubbed.
const SKILLS: [&str; 3] = ["auto-drive-to-merge", "crusty-old-engineer", "merge-ready"];

/// Copies the checkout's bundle into `to` without the skills a drive does not
/// read. Those skills are most of the bundle's files, and each drive copies and
/// hashes its whole bundle when it starts, so leaving them out halves the time
/// of the drive tests.
fn stage_bundle(to: &Path) {
    let from = root().join("amplifier-bundle");
    let skills = from.join("skills");
    copy_bundle(&from, to, &|path: &Path| {
        path.parent() != Some(skills.as_path()) || SKILLS.iter().any(|s| path.ends_with(s))
    });
}

fn copy_bundle(from: &Path, to: &Path, keep: &dyn Fn(&Path) -> bool) {
    fs::create_dir_all(to).unwrap();
    fs::set_permissions(to, fs::Permissions::from_mode(0o755)).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if !keep(&entry.path()) {
            continue;
        }
        let kind = entry.file_type().unwrap();
        let dest = to.join(entry.file_name());
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &dest).unwrap();
        } else if kind.is_dir() {
            copy_bundle(&entry.path(), &dest, keep);
        } else {
            fs::copy(entry.path(), &dest).unwrap();
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o755;
            fs::set_permissions(&dest, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}

fn read_yaml(path: &Path) -> Yaml {
    serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn write_yaml(path: &Path, doc: &Yaml) {
    fs::write(path, serde_yaml::to_string(doc).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn write_private(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// The `command` of step `id` in a recipe of the checkout's bundle.
fn step_command(recipe: &str, id: &str) -> String {
    let doc = read_yaml(&root().join("amplifier-bundle/recipes").join(recipe));
    doc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{recipe} has no step {id}"))["command"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Replaces every agent step of the recipe with a bash step. `reply(id)` gives
/// the step's one-line output, or `None` for `{}`.
fn stub_agents(path: &Path, reply: &dyn Fn(&str) -> Option<String>) {
    let mut doc = read_yaml(path);
    for step in doc["steps"].as_sequence_mut().unwrap() {
        if step["type"].as_str() != Some("agent") {
            continue;
        }
        let id = step["id"].as_str().unwrap().to_owned();
        let mut stub = serde_yaml::Mapping::new();
        for key in ["id", "condition", "output", "parse_json"] {
            if let Some(v) = step.get(key) {
                stub.insert(key.into(), v.clone());
            }
        }
        stub.insert("type".into(), "bash".into());
        let out = reply(&id).unwrap_or_else(|| "{}".to_owned());
        stub.insert("command".into(), format!("echo '{out}'").into());
        *step = Yaml::Mapping(stub);
    }
    write_yaml(path, &doc);
}

fn assert_no_agent_step(path: &Path) {
    let doc = read_yaml(path);
    for step in doc["steps"].as_sequence().unwrap() {
        assert_ne!(
            step["type"].as_str(),
            Some("agent"),
            "{} still has agent step {:?}",
            path.display(),
            step["id"]
        );
    }
}

const FAKE_GH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FAKE_LOG/gh.log"
head="$(cat "$FAKE_LOG/head")"
case "${1:-} ${2:-}" in
  "pr view")
    case "$*" in *isDraft*) echo false; exit 0 ;; esac
    state=OPEN; [ -e "$FAKE_LOG/merged" ] && state=MERGED
    jq -cn --arg h "$head" --arg s "$state" '{state:$s,title:"Fixture",body:"Fixture body",mergeStateStatus:"CLEAN",headRefOid:$h}' ;;
  "pr checks") printf 'build\tpass\t1m\thttps://example.invalid\n' ;;
  "api repos/{owner}/{repo}") printf 'true\ttrue\ttrue\n' ;;
  "pr merge") printf '%s\n' "$@" > "$FAKE_LOG/merge-argv"; cat > /dev/null; : > "$FAKE_LOG/merged" ;;
  *) echo "fixture gh: unexpected call: $*" >&2; exit 1 ;;
esac
"#;

/// The review's records, written into the folder the driver is given. The
/// driver records its argv so a test can see which folder it was handed.
const FAKE_DRIVER: &str = r#"#!/usr/bin/env bash
loop=""; sd=""; args=("$@")
while [ $# -gt 0 ]; do
  case "$1" in
    --loop-name) loop="$2"; shift 2 ;;
    --state-dir) sd="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '%s\n' "${args[@]}" > "$FAKE_LOG/driver-$loop.args"
head="$(cat "$FAKE_LOG/head")"
umask 077
mkdir -p -m 700 -- "$sd"
case "$loop" in
  crusty)
    printf 'CLEAN\n' > "$sd/crusty-round-2.verdict"
    printf 'crusty-loop\tdone\n' >> "$sd/phases.tsv"
    echo '{"loop":"crusty","loop_result":"DONE","round_label":"round-2"}' ;;
  merge-ready)
    printf '%s\n' "$head" > "$sd/merge-ready-latest.head"
    printf 'MERGE_READY\n' > "$sd/merge-ready-round-1.verdict"
    r='{"loop":"merge-ready","loop_result":"DONE","round_label":"round-1"}'
    printf '%s\n' "$r" > "$sd/merge-ready-result.json"
    echo "$r" ;;
esac
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    trusted: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    repo: PathBuf,
    /// The drive's work root (`AMPLIHACK_WORK_ROOT`).
    work: PathBuf,
    /// The folder every agent step reports: the work root's per-PR folder,
    /// as #1516's build reported.
    reported: PathBuf,
    /// The folder the runner binds.
    bound: PathBuf,
    head: String,
}

impl Fixture {
    fn bundle(&self) -> PathBuf {
        self.trusted.join("amplifier-bundle")
    }

    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let trusted = base.join("trusted");
        let bin = base.join("bin");
        let log = base.join("log");
        let clone = base.join("clone");
        let work = base.join("work");
        let reported = work
            .join("autodrive")
            .join(format!("github.com__owner__fixture__pr-{PR}"));
        let bound = base.join("bound-B");
        for d in [&home, &bin, &log, &clone] {
            fs::create_dir_all(d).unwrap();
        }
        private_dir(&work);
        private_dir(&work.join("autodrive"));
        private_dir(&reported);
        private_dir(&bound);
        stage_bundle(&trusted.join("amplifier-bundle"));

        let git = |dir: &Path, args: &[&str]| -> String {
            let o = amplihack_git::command_in(dir)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                ])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(o.status.success(), "git {args:?}: {o:?}");
            String::from_utf8_lossy(&o.stdout).trim().to_owned()
        };
        git(&clone, &["init", "-q", "-b", "main"]);
        fs::write(clone.join("README.md"), "fixture\n").unwrap();
        git(&clone, &["add", "README.md"]);
        git(&clone, &["commit", "-qm", "fixture"]);
        // #1516 launched from a linked worktree at <clone>/worktrees/pr-N.
        let repo = clone.join("worktrees").join(format!("pr-{PR}"));
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/issue-3053",
                repo.to_str().unwrap(),
            ],
        );
        let head = git(&repo, &["rev-parse", "HEAD"]);
        fs::write(log.join("head"), &head).unwrap();

        write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        write_executable(
            &bin.join("codex"),
            "#!/bin/sh\necho 'fixture codex: no agent runs in this test' >&2\nexit 1\n",
            0o755,
        );
        std::os::unix::fs::symlink(amplihack_bin(), bin.join("amplihack")).unwrap();
        write_executable(
            &trusted.join("amplifier-bundle/tools/autodrive_loop.sh"),
            FAKE_DRIVER,
            0o755,
        );
        Fixture {
            _tmp: tmp,
            base,
            home,
            trusted,
            bin,
            log,
            repo,
            work,
            reported,
            bound,
            head,
        }
    }

    /// The preflight reply of the loop recipes: folder A, the PR worktree.
    fn preflight(&self) -> String {
        json!({
            "should_run": "true",
            "pr": PR,
            "repo": self.repo,
            "state_dir": self.reported,
            "base_branch": "main"
        })
        .to_string()
    }

    fn build_result(&self) -> String {
        json!({
            "pr": PR,
            "pr_url": format!("https://example.invalid/pull/{PR}"),
            "head_sha": self.head,
            "base_sha": self.head,
            "is_cross_repository": false,
            "merged_before_run": "false",
            "state_dir": self.reported,
        })
        .to_string()
    }

    /// Stubs every agent step of the drive recipes, replaces the build recipe
    /// with a stand-in that reports folder A, and asserts that no agent
    /// step remains.
    fn stage_recipes(&self) {
        let recipes = self.bundle().join("recipes");
        let pre = self.preflight();
        let build = self.build_result();
        let reply = move |id: &str| -> Option<String> {
            match id {
                "step-01-crusty-loop-preflight" | "step-01-merge-loop-preflight" => {
                    Some(pre.clone())
                }
                "light-resolve-pr" => Some(build.clone()),
                "step-03b-base-sync" => Some(r#"{"synced":"false"}"#.into()),
                "step-03-record-crusty-phase" => Some(r#"{"recorded":true}"#.into()),
                _ => None,
            }
        };
        for name in [
            "auto-drive-to-merge",
            "auto-drive-light",
            "autodrive-crusty-loop",
            "autodrive-merge-loop",
            // The finalisation and hand-off agents only narrate; their bash
            // steps stay real.
            "autodrive-finalize",
            "autodrive-handoff",
        ] {
            let path = recipes.join(format!("{name}.yaml"));
            stub_agents(&path, &reply);
            assert_no_agent_step(&path);
        }
        let standin = format!(
            r#"name: autodrive-build
description: stand-in build that reports folder A
version: 1.0.0
context:
  step_recovery_dir: ""
  base_branch: ""
  task_description: ""
  requirements: ""
  repo_path: ""
  branch_prefix: ""
  existing_branch: ""
  pr_number: ""
  pr_url: ""
  autodrive_state_dir: ""
  no_merge: ""
  autodrive_pr_input: ""
  autodrive_dry_run: ""
  build_result: ""
steps:
  - id: report-build
    type: bash
    parse_json: true
    output: build_result
    command: |
      echo '{}'
"#,
            self.build_result().replace('\'', "")
        );
        let path = recipes.join("autodrive-build.yaml");
        fs::write(&path, standin).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }

    fn command(&self) -> Command {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.trusted)
            .env(
                "RECIPE_RUNNER_RS_PATH",
                crate::issue_2539_step_recovery_fixture::real_runner(),
            )
            .env("PATH", path)
            .env("FAKE_LOG", &self.log)
            .env("AMPLIHACK_WORK_ROOT", &self.work)
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1");
        c
    }

    /// Runs a drive recipe by name against the copy, with folder B bound.
    fn drive(&self, recipe: &str, extra: &[&str]) -> Output {
        let bind = format!("autodrive_state_dir={}", self.bound.display());
        self.drive_given(recipe, &[&[bind.as_str()], extra].concat())
    }

    /// Runs a drive recipe by name against the copy with only the `-c`
    /// values given, so with no `autodrive_state_dir` the runner binds its
    /// own folder, as on #1516.
    fn drive_given(&self, recipe: &str, extra: &[&str]) -> Output {
        let mut c = self.command();
        c.args(["recipe", "run", recipe])
            .args(["-c", &format!("pr_number={PR}")])
            .args(["-c", "autodrive_dry_run=false"])
            .args(["-c", "task_description=fixture task"]);
        for kv in extra {
            c.args(["-c", kv]);
        }
        c.args(["--format", "json", "-w"])
            .arg(&self.repo)
            .current_dir(&self.repo)
            .output()
            .expect("amplihack recipe run")
    }

    /// A plain command with the real `amplihack` on PATH and no inherited
    /// recipe variables or tokens. Its scratch files go under the fixture,
    /// never the shared /tmp.
    fn command_plain(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("TMPDIR", &self.base)
            .env("AMPLIHACK_HOME", &self.trusted)
            .env("FAKE_LOG", &self.log)
            .env("REPO_PATH", &self.repo)
            .current_dir(&self.repo);
        c
    }

    /// Step `id`'s own command under `bash -c`, with the stand-in `amplihack`
    /// on PATH, folder B bound and a step-recovery folder. The caller adds the
    /// preflight the step reads.
    fn standin_step(&self, recipe: &str, id: &str) -> Command {
        let standin = self.base.join("standin-bin");
        fs::create_dir_all(&standin).unwrap();
        write_executable(&standin.join("amplihack"), STANDIN_AMPLIHACK, 0o755);
        let recovery = self.base.join("recovery");
        private_dir(&recovery);
        let mut c = Command::new("bash");
        c.arg("-c")
            .arg(step_command(recipe, id))
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", standin.display()))
            .env("HOME", &self.home)
            .env("TMPDIR", &self.base)
            .env("AMPLIHACK_HOME", &self.trusted)
            .env("FAKE_LOG", &self.log)
            .env("FAKE_REPORTED", &self.reported)
            .env("FAKE_REPO", &self.repo)
            .env("FAKE_PR", PR)
            .env("RECIPE_VAR_autodrive_state_dir", &self.bound)
            .env("RECIPE_VAR_step_recovery_dir", &recovery);
        c
    }

    fn files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn driver_args(&self, loop_name: &str) -> Vec<String> {
        fs::read_to_string(self.log.join(format!("driver-{loop_name}.args")))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// `finalize-merge.json` in folder B.
    fn merge_record(&self) -> Value {
        let raw = fs::read_to_string(self.bound.join("finalize-merge.json"))
            .unwrap_or_else(|e| panic!("no merge record in the bound folder: {e}"));
        serde_json::from_str(raw.trim()).unwrap()
    }

    fn assert_reported_folder_untouched(&self, out: &Output) {
        assert!(
            Self::files(&self.reported).is_empty(),
            "the folder an agent step reported was written to: {:?}\n{}",
            Self::files(&self.reported),
            show(out)
        );
    }
}

fn show(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let i = argv.iter().position(|a| a == flag)?;
    argv.get(i + 1).map(String::as_str)
}

fn outcome(out: &Output) -> Value {
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("structured run result ({e}): {}", show(out)));
    v["autodrive_outcome"].clone()
}

// ---------------------------------------------------------------------------
// T1 (#3053): a full drive whose build reports folder A while B is bound.
// On main the crusty loop gets A, the write-back replaces the drive's folder,
// the merge loop and finalisation then use A, and the helper refuses with
// authorization-unreadable.
#[test]
fn a_drive_whose_build_reports_another_folder_still_merges_from_the_bound_one() {
    let f = Fixture::new();
    f.stage_recipes();
    let out = f.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    f.assert_reported_folder_untouched(&out);

    let crusty = f.driver_args("crusty");
    assert_eq!(
        flag_value(&crusty, "--state-dir"),
        f.bound.to_str(),
        "{crusty:?}"
    );
    let merge = f.driver_args("merge-ready");
    assert_eq!(
        flag_value(&merge, "--state-dir"),
        f.bound.to_str(),
        "{merge:?}"
    );
    assert!(
        merge
            .iter()
            .any(|a| a == &format!("autodrive_state_dir={}", f.bound.display())),
        "{merge:?}"
    );
    assert!(
        f.bound.join("crusty-round-2.verdict").exists(),
        "{}",
        show(&out)
    );
    assert!(
        f.bound.join("merge-ready-latest.head").exists(),
        "{}",
        show(&out)
    );

    let argv: Vec<String> = fs::read_to_string(f.log.join("merge-argv"))
        .unwrap_or_else(|_| panic!("the merge was not authorised: {}", show(&out)))
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        flag_value(&argv, "--match-head-commit"),
        Some(f.head.as_str())
    );
    let record = f.merge_record();
    let o = outcome(&out);
    assert_eq!(record["result"], json!("merged"), "{record}");
    assert_eq!(record["run_id"], o["run_id"], "{record}\n{o}");
    assert_eq!(o["requested_action"], json!("merge"), "{o}");
    assert_eq!(o["stop_reason"], json!("completed"), "{o}");
}

// ---------------------------------------------------------------------------
// T2a (#2997): auto-drive-light allowed to merge. Fails on main at the helper:
// the merge loop is handed A while the invocation binds B.
#[test]
fn a_light_drive_allowed_to_merge_finds_the_review_records_in_the_bound_folder() {
    let f = Fixture::new();
    f.stage_recipes();
    let out = f.drive("auto-drive-light", &["no_merge=false"]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    f.assert_reported_folder_untouched(&out);
    assert!(
        f.bound.join("light-eligibility.json").exists(),
        "{}",
        show(&out)
    );
    assert!(
        f.log.join("merge-argv").exists(),
        "no merge: {}",
        show(&out)
    );
    let record = f.merge_record();
    assert_eq!(record["result"], json!("merged"), "{record}");
}

// T2b (#2997): light's default is verify-only. The run ends completed with
// finalisation not-authorised (verify-only), never unreadable. Fails on main
// in the report, which compares the build's folder with the bound one.
#[test]
fn a_light_drive_left_verify_only_completes_from_the_bound_folder() {
    let f = Fixture::new();
    f.stage_recipes();
    let out = f.drive("auto-drive-light", &[]);
    f.assert_reported_folder_untouched(&out);
    assert!(!f.log.join("merge-argv").exists(), "{}", show(&out));
    let record = f.merge_record();
    assert_eq!(record["result"], json!("not-authorised"), "{record}");
    assert_eq!(
        record["reason"],
        json!("this run is verify-only"),
        "{record}"
    );
    let o = outcome(&out);
    assert_eq!(o["stop_reason"], json!("completed"), "{o}\n{}", show(&out));
}

// T2e (#2997): light's escalation hand-off to auto-drive-to-merge. The copy's
// auto-drive-to-merge records the folder it is given.
#[test]
fn a_light_escalation_hands_the_bound_folder_to_the_full_drive() {
    let f = Fixture::new();
    f.stage_recipes();
    let drive = f.bundle().join("recipes/auto-drive-to-merge.yaml");
    let mut doc = read_yaml(&drive);
    doc["steps"] = serde_yaml::from_str(
        r#"- id: record
  type: bash
  command: printf '%s' "$RECIPE_VAR_autodrive_state_dir" > "$FAKE_LOG/escalated-state-dir"
"#,
    )
    .unwrap();
    write_yaml(&drive, &doc);
    // An escalation marker in B makes light's eligibility decide "escalate".
    write_private(&f.bound.join("escalation.json"), "{}\n");
    let out = f.drive("auto-drive-light", &[]);
    f.assert_reported_folder_untouched(&out);
    let seen = fs::read_to_string(f.log.join("escalated-state-dir"))
        .unwrap_or_else(|_| panic!("light did not escalate: {}", show(&out)));
    assert_eq!(seen, f.bound.to_str().unwrap(), "{}", show(&out));
}

// ---------------------------------------------------------------------------
// T5 (#3053, as on #1516): the operator gives no folder, so the runner binds
// <work root>/runs/<id>/autodrive-state, and the build reports the work root's
// per-PR folder. On main the crusty loop is handed the build's folder, which
// then replaces the bound one, and finalisation ends authorization-unreadable.
#[test]
fn a_drive_given_no_folder_keeps_every_record_in_the_one_the_runner_bound() {
    for recipe in ["auto-drive-to-merge", "auto-drive-light"] {
        let f = Fixture::new();
        f.stage_recipes();
        let out = f.drive_given(recipe, &["no_merge=false"]);
        assert_eq!(out.status.code(), Some(0), "{recipe}: {}", show(&out));
        f.assert_reported_folder_untouched(&out);

        let crusty = f.driver_args("crusty");
        let merge = f.driver_args("merge-ready");
        let bound = PathBuf::from(
            flag_value(&crusty, "--state-dir")
                .unwrap_or_else(|| panic!("{recipe}: no crusty review: {}", show(&out))),
        );
        assert_eq!(
            flag_value(&merge, "--state-dir"),
            bound.to_str(),
            "{recipe}: {merge:?}"
        );
        assert!(
            bound.starts_with(f.work.join("runs")) && bound.ends_with("autodrive-state"),
            "{recipe}: not the runner's own folder: {}",
            bound.display()
        );
        for record in ["crusty-round-2.verdict", "merge-ready-latest.head"] {
            assert!(bound.join(record).exists(), "{recipe}: no {record}");
        }

        let argv: Vec<String> = fs::read_to_string(f.log.join("merge-argv"))
            .unwrap_or_else(|_| panic!("{recipe}: the merge was not authorised: {}", show(&out)))
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            flag_value(&argv, "--match-head-commit"),
            Some(f.head.as_str()),
            "{recipe}: {argv:?}"
        );
        let raw = fs::read_to_string(bound.join("finalize-merge.json"))
            .unwrap_or_else(|e| panic!("{recipe}: no merge record in the bound folder: {e}"));
        let record: Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(record["result"], json!("merged"), "{recipe}: {record}");
        assert_eq!(
            outcome(&out)["stop_reason"],
            json!("completed"),
            "{recipe}: {}",
            show(&out)
        );
    }
}

// ---------------------------------------------------------------------------
// Direct-command tests: one step's own command, with the runner's variable set
// to B and the preflight or build reporting A.

/// A stand-in `amplihack`: `extract-json` passes its input through,
/// `extract-field` answers from the fixture, `step-recovery run` runs the
/// command after `--`, and `recipe run` records its arguments.
const STANDIN_AMPLIHACK: &str = r#"#!/usr/bin/env bash
case "$1 $2 $3" in
  "orch helper extract-json") cat ;;
  "orch helper extract-field")
    cat > /dev/null
    case "$5" in
      state_dir) printf '%s' "$FAKE_REPORTED" ;;
      repo) printf '%s' "$FAKE_REPO" ;;
      pr) printf '%s' "$FAKE_PR" ;;
    esac ;;
  "orch helper step-recovery")
    while [ "$1" != "--" ]; do shift; done; shift; exec "$@" ;;
  "recipe run autodrive-finalize")
    printf '%s\n' "$@" > "$FAKE_LOG/finalize-args"; echo '{}' ;;
  *) echo "stand-in amplihack: unexpected: $*" >&2; exit 1 ;;
esac
"#;

// T3 (#2996): step-03r's re-run gets the bound folder, not the preflight's.
// Fails on main: the re-run is given the preflight's folder A.
#[test]
fn the_finalize_recovery_rerun_gets_the_bound_folder() {
    let f = Fixture::new();
    let out = f
        .standin_step("autodrive-merge-loop.yaml", "step-03r-finalize-recovery")
        .env("RECIPE_VAR_merge_loop_preflight", f.preflight())
        .output()
        .unwrap();
    let args = fs::read_to_string(f.log.join("finalize-args"))
        .unwrap_or_else(|_| panic!("no re-run: {}", show(&out)));
    assert!(
        args.lines()
            .any(|l| l == format!("autodrive_state_dir={}", f.bound.display())),
        "{args}"
    );
    assert!(!args.contains(f.reported.to_str().unwrap()), "{args}");
}

// T4a: the merge loop tool (step-02 and step-03c both run it).
#[test]
fn the_merge_loop_tool_hands_the_driver_the_bound_folder() {
    let f = Fixture::new();
    let out = f
        .command_plain("bash")
        .arg(f.bundle().join("tools/autodrive_merge_loop_run.sh"))
        .env("RECIPE_VAR_merge_loop_preflight", f.preflight())
        .env("RECIPE_VAR_autodrive_state_dir", &f.bound)
        .env("RECIPE_VAR_task_description", "fixture task")
        .output()
        .unwrap();
    let args = f.driver_args("merge-ready");
    assert!(!args.is_empty(), "the driver did not run: {}", show(&out));
    assert_eq!(
        flag_value(&args, "--state-dir"),
        f.bound.to_str(),
        "{args:?}"
    );
    assert!(
        args.iter()
            .any(|a| a == &format!("autodrive_state_dir={}", f.bound.display())),
        "{args:?}"
    );
    f.assert_reported_folder_untouched(&out);
}

// T4b: crusty step-02.
#[test]
fn the_crusty_loop_step_hands_the_driver_the_bound_folder() {
    let f = Fixture::new();
    let out = f
        .command_plain("bash")
        .arg("-c")
        .arg(step_command(
            "autodrive-crusty-loop.yaml",
            "step-02-crusty-loop",
        ))
        .env("RECIPE_VAR_crusty_loop_preflight", f.preflight())
        .env("RECIPE_VAR_autodrive_state_dir", &f.bound)
        .env("RECIPE_VAR_task_description", "fixture task")
        .output()
        .unwrap();
    let args = f.driver_args("crusty");
    assert!(!args.is_empty(), "the driver did not run: {}", show(&out));
    assert_eq!(
        flag_value(&args, "--state-dir"),
        f.bound.to_str(),
        "{args:?}"
    );
    f.assert_reported_folder_untouched(&out);
}

// T4c: crusty step-03r checks the phase in the bound folder, in both
// directions. Fails on main: it checks the preflight's folder.
#[test]
fn the_crusty_phase_check_reads_the_bound_folder_only() {
    let run = |recorded_in_bound: bool| -> Output {
        let f = Fixture::new();
        let recorded = if recorded_in_bound {
            &f.bound
        } else {
            &f.reported
        };
        write_private(&recorded.join("phases.tsv"), "crusty-loop\tdone\n");
        f.standin_step(
            "autodrive-crusty-loop.yaml",
            "step-03r-record-crusty-phase-recovery",
        )
        .env("RECIPE_VAR_crusty_loop_preflight", f.preflight())
        .output()
        .unwrap()
    };
    let in_bound = run(true);
    assert_eq!(in_bound.status.code(), Some(0), "{}", show(&in_bound));
    let in_reported = run(false);
    assert_ne!(in_reported.status.code(), Some(0), "{}", show(&in_reported));
}

// T4d: run directly, outside amplihack, with no bound folder, the readers
// fall back to the folder the preflight reported. Passes on main by design.
#[test]
fn a_direct_run_with_no_bound_folder_uses_the_reported_one() {
    let f = Fixture::new();
    let out = f
        .command_plain("bash")
        .arg("-c")
        .arg(step_command(
            "autodrive-crusty-loop.yaml",
            "step-02-crusty-loop",
        ))
        .env("RECIPE_VAR_crusty_loop_preflight", f.preflight())
        .env("RECIPE_VAR_task_description", "fixture task")
        .output()
        .unwrap();
    let args = f.driver_args("crusty");
    assert!(!args.is_empty(), "the driver did not run: {}", show(&out));
    assert_eq!(
        flag_value(&args, "--state-dir"),
        f.reported.to_str(),
        "{args:?}"
    );
}
