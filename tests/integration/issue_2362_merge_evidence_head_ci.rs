//! tests/integration/issue_2362_merge_evidence_head_ci.rs
//!
//! Issue #2362: auto-drive-to-merge's merge-ready QA ran before the CI run at
//! the head had finished, and every round merged the base and pushed, which
//! restarted CI. These tests run the step bodies of
//! `amplifier-bundle/recipes/autodrive-merge-evidence.yaml` in YAML order, as
//! the recipe runner does, and check what they do:
//!
//! - QA starts only after `gh run watch` has returned for the run at the head
//!   (an event, no timer), and the final `ci_evidence` comes from that wait.
//! - `ci_evidence` is bound to the local `HEAD`, maps run conclusions to
//!   GREEN, RED, PENDING, NO_CHECKS or UNREADABLE, and is never GREEN when it
//!   cannot read the runs. PENDING (no run registered at the head yet) keeps
//!   the signal `no-run-for-head`, so two such rounds at one head stop as
//!   STUCK rather than looping.
//! - Every `gh run watch` passes `--interval N`, N from the caller setting
//!   `autodrive_ci_watch_interval` (default 120, minimum 60; anything else
//!   falls back to 120 with a note on stderr). The runs at the head are
//!   watched one at a time, and one round makes the minimum number of gh
//!   calls (bb:0 rate-limit incident, 11 Nov).
//! - The setting reaches step-01b from a whole-drive `-c` through the merge
//!   loop and the round.
//! - Step-01 merges and pushes only when GitHub's `mergeStateStatus` requires
//!   it (`BEHIND`, or `DIRTY` with a clean local merge).
//!
//! Every test is hermetic: a temporary HOME, AMPLIHACK_HOME and gh config, a
//! local bare `origin`, a stub `gh` that never calls the real binary, a QA
//! stub, no network. Waits use FIFOs, never sleeps or deadlines.

#![cfg(unix)]
#![allow(clippy::too_many_lines)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::Value as Json;
use serde_yaml::Value;
use tempfile::TempDir;

const RECIPE: &str = "autodrive-merge-evidence.yaml";
const SYNC_STEP: &str = "step-01-sync-with-base";
const WAIT_STEP: &str = "step-01b-wait-for-head-ci";
const PR: &str = "42";
/// The caller setting read by step-01b, and the two names the runner gives
/// it in a step's environment.
const INTERVAL_KEY: &str = "autodrive_ci_watch_interval";
const INTERVAL_VAR: &str = "RECIPE_VAR_autodrive_ci_watch_interval";
const INTERVAL_ALIAS: &str = "AUTODRIVE_CI_WATCH_INTERVAL";
const DEFAULT_INTERVAL: &str = "120";

// ---------------------------------------------------------------------------
// Recipe
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

/// `(id, output, command)` for every bash step, in YAML order.
fn recipe_steps() -> Vec<(String, String, String)> {
    let path = workspace_root()
        .join("amplifier-bundle/recipes")
        .join(RECIPE);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let yaml: Value =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    yaml.get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe has steps")
        .iter()
        .map(|step| {
            let field = |name: &str| {
                step.get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            (field("id"), field("output"), field("command"))
        })
        .collect()
}

/// The steps whose ids are in `ids`, in YAML order. A missing id fails the
/// test by name.
fn select_steps(ids: &[&str]) -> Vec<(String, String, String)> {
    let all = recipe_steps();
    for id in ids {
        assert!(
            all.iter().any(|(sid, _, _)| sid == id),
            "{RECIPE} has no step `{id}` (issue #2362); steps are {:?}",
            all.iter()
                .map(|(sid, _, _)| sid.as_str())
                .collect::<Vec<_>>()
        );
    }
    all.into_iter()
        .filter(|(id, _, _)| ids.contains(&id.as_str()))
        .collect()
}

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

/// A stub `gh` driven by files in `$STUB_GH_STATE`. It appends its argv to
/// `calls.log` and answers:
/// - `pr view`: `{baseRefName, mergeStateStatus, number}` from `base` and
///   `merge_state`; `pr_view_fail` makes it exit 1.
/// - `run list`: `runs/<commit>.json` for the `--commit` value, else `[]`;
///   `run_list_fail` makes it exit 1.
/// - `workflow list`: `workflows.json`, else `[]`; `workflow_list_fail` fails.
/// - `run watch <id>`: when `block_watch` exists, writes `started` to the
///   `started` FIFO and blocks reading the `release` FIFO. Exits with
///   `watch_exit/<id>`, else 0.
/// - `run view <id>`: `views/<id>.json`, else exit 1.
///
/// `--jq`/`-q` filters are applied with the host jq. Anything else exits 64.
const STUB_GH: &str = r#"#!/usr/bin/env bash
state="${STUB_GH_STATE:?}"
printf '%s\n' "$*" >> "$state/calls.log"
query=""; commit=""; prev=""; id=""; n=0
for a in "$@"; do
  n=$((n + 1))
  case "$prev" in
    --jq|-q) query="$a" ;;
    --commit|-c) commit="$a" ;;
  esac
  if [ "$n" -gt 2 ] && [ -z "$id" ]; then
    case "$a" in ''|*[!0-9]*) ;; *) case "$prev" in --limit|-L) ;; *) id="$a" ;; esac ;; esac
  fi
  prev="$a"
done
emit() {
  if [ -n "$query" ]; then printf '%s' "$1" | jq -r "$query"; else printf '%s\n' "$1"; fi
}
case "${1:-} ${2:-}" in
  "pr view")
    [ -e "$state/pr_view_fail" ] && { echo "stub gh: pr view failing as asked" >&2; exit 1; }
    base="main"; [ -f "$state/base" ] && base="$(cat "$state/base")"
    ms="CLEAN"; [ -f "$state/merge_state" ] && ms="$(cat "$state/merge_state")"
    emit "$(jq -cn --arg b "$base" --arg m "$ms" '{baseRefName:$b,mergeStateStatus:$m,number:42}')"
    ;;
  "run list")
    [ -e "$state/run_list_fail" ] && { echo "stub gh: run list failing as asked" >&2; exit 1; }
    if [ -n "$commit" ] && [ -f "$state/runs/$commit.json" ]; then
      emit "$(cat "$state/runs/$commit.json")"
    else
      emit '[]'
    fi
    ;;
  "workflow list")
    [ -e "$state/workflow_list_fail" ] && { echo "stub gh: workflow list failing as asked" >&2; exit 1; }
    if [ -f "$state/workflows.json" ]; then emit "$(cat "$state/workflows.json")"; else emit '[]'; fi
    ;;
  "run watch")
    if [ -e "$state/block_watch" ]; then
      printf 'started\n' > "$state/started"
      read -r _ < "$state/release"
    fi
    code=0; [ -f "$state/watch_exit/$id" ] && code="$(cat "$state/watch_exit/$id")"
    printf 'stub gh: run %s finished\n' "$id"
    exit "$code"
    ;;
  "run view")
    [ -f "$state/views/$id.json" ] || { echo "stub gh: no run $id" >&2; exit 1; }
    emit "$(cat "$state/views/$id.json")"
    ;;
  *)
    echo "stub gh: unsupported call: $*" >&2
    exit 64
    ;;
esac
"#;

/// QA stub found by step-02 under `$AMPLIHACK_HOME/amplifier-bundle/tools`.
/// It records the head it judged in the same log as the gh calls.
const STUB_QA: &str = r#"#!/usr/bin/env bash
printf 'qa:%s\n' "$(git -C "${REPO_PATH:-.}" rev-parse HEAD 2>/dev/null)" >> "${STUB_GH_STATE:?}/calls.log"
printf '{"qa_status":"PASS","gadugi_status":"NOT_REQUIRED"}\n'
"#;

/// `amplihack` is never the real binary here; step-02's base lookup falls
/// back to an empty base when it fails.
const STUB_AMPLIHACK: &str = "#!/usr/bin/env bash\nexit 1\n";

fn write_exec(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

struct Sandbox {
    _tmp: TempDir,
    root: PathBuf,
}

/// What one step printed.
struct StepResult {
    code: String,
    stdout: String,
    stderr: String,
}

impl StepResult {
    fn json(&self, id: &str) -> Json {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "step `{id}` must print exactly one JSON object on stdout ({e})\nexit {}\nstdout:\n{}\nstderr:\n{}",
                self.code, self.stdout, self.stderr
            )
        })
    }
}

impl Sandbox {
    /// A bare `origin` with `main` (base.txt) and `feat` (one more commit),
    /// a `seed` clone used to move `origin`, and the `work` clone on `feat`
    /// tracking `origin/feat`, where the steps run.
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
        for dir in [
            "home",
            "bin",
            "gh-config",
            "gh-state/runs",
            "gh-state/views",
            "gh-state/watch_exit",
            "amp-home",
            "steps",
            "out",
        ] {
            fs::create_dir_all(root.join(dir)).expect("create sandbox dir");
        }
        write_exec(&root.join("bin/gh"), STUB_GH);
        write_exec(&root.join("bin/amplihack"), STUB_AMPLIHACK);
        write_exec(
            &root.join("amp-home/amplifier-bundle/tools/autodrive_qa_evidence.sh"),
            STUB_QA,
        );
        // #2033: steps that run git source the repository-env helper from
        // `$AMPLIHACK_HOME/amplifier-bundle/tools`. The sandbox gets this
        // workspace's copy, so the real ~/.amplihack is never read.
        let helper = workspace_root().join("amplifier-bundle/tools/git-repo-env.sh");
        if helper.is_file() {
            let dest = root.join("amp-home/amplifier-bundle/tools/git-repo-env.sh");
            fs::copy(&helper, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", helper.display()));
        }
        let sb = Self { _tmp: tmp, root };

        let origin = sb.root.join("origin.git");
        sb.git(
            &sb.root,
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        let seed = sb.seed();
        sb.git(&sb.root, &["init", "-b", "main", seed.to_str().unwrap()]);
        fs::write(seed.join("base.txt"), "base\n").unwrap();
        sb.git(&seed, &["add", "."]);
        sb.git(&seed, &["commit", "-q", "-m", "base"]);
        sb.git(
            &seed,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        sb.git(&seed, &["push", "-q", "origin", "main"]);
        sb.git(&seed, &["checkout", "-q", "-b", "feat"]);
        fs::write(seed.join("feat.txt"), "feat\n").unwrap();
        sb.git(&seed, &["add", "."]);
        sb.git(&seed, &["commit", "-q", "-m", "feat"]);
        sb.git(&seed, &["push", "-q", "origin", "feat"]);
        sb.git(&seed, &["checkout", "-q", "main"]);

        sb.git(
            &sb.root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                sb.work().to_str().unwrap(),
            ],
        );
        sb.git(
            &sb.work(),
            &["checkout", "-q", "-b", "feat", "--track", "origin/feat"],
        );
        sb
    }

    fn seed(&self) -> PathBuf {
        self.root.join("seed")
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    fn origin(&self) -> PathBuf {
        self.root.join("origin.git")
    }

    fn state(&self) -> PathBuf {
        self.root.join("gh-state")
    }

    fn isolate<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        amplihack_git::scrub(command)
            .env("HOME", self.root.join("home"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("AMPLIHACK_HOME", self.root.join("amp-home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Auto Drive Test")
            .env("GIT_AUTHOR_EMAIL", "auto-drive@example.invalid")
            .env("GIT_COMMITTER_NAME", "Auto Drive Test")
            .env("GIT_COMMITTER_EMAIL", "auto-drive@example.invalid")
            .env("GH_CONFIG_DIR", self.root.join("gh-config"))
            .env("STUB_GH_STATE", self.state())
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_HOST")
            .env_remove("GH_REPO")
            .env_remove("GH_ENTERPRISE_TOKEN")
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("REPO_PATH")
            .env_remove("PR_NUMBER")
    }

    fn git_output(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = amplihack_git::command();
        command.args(args).current_dir(dir);
        self.isolate(&mut command)
            .output()
            .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()))
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.git_output(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} in {}: exit {:?}\n{}\n{}",
            dir.display(),
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        );
        text(&output.stdout).trim().to_string()
    }

    fn head(&self) -> String {
        self.git(&self.work(), &["rev-parse", "HEAD"])
    }

    fn origin_ref(&self, branch: &str) -> String {
        self.git(
            &self.origin(),
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
    }

    /// Commit `content` to `file` on origin's `main`.
    fn advance_main(&self, file: &str, content: &str) {
        let seed = self.seed();
        self.git(&seed, &["checkout", "-q", "main"]);
        fs::write(seed.join(file), content).unwrap();
        self.git(&seed, &["add", "."]);
        self.git(&seed, &["commit", "-q", "-m", &format!("main: {file}")]);
        self.git(&seed, &["push", "-q", "origin", "main"]);
    }

    /// Commit `content` to `file` on the work branch and push it.
    fn commit_feat(&self, file: &str, content: &str) {
        let work = self.work();
        fs::write(work.join(file), content).unwrap();
        self.git(&work, &["add", "."]);
        self.git(&work, &["commit", "-q", "-m", &format!("feat: {file}")]);
        self.git(&work, &["push", "-q"]);
    }

    fn set(&self, name: &str, content: &str) {
        let path = self.state().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }

    fn merge_state(&self, value: &str) {
        self.set("merge_state", value);
    }

    fn active_workflow(&self) {
        self.set("workflows.json", r#"[{"name":"CI","state":"active"}]"#);
    }

    /// Runs listed for `sha`: `(id, headSha)`, each with a view of
    /// `(conclusion, view headSha)`.
    fn runs(&self, sha: &str, runs: &[(&str, &str, &str, &str)]) {
        let list: Vec<Json> = runs
            .iter()
            .map(|(id, head, _, _)| {
                serde_json::json!({
                    "databaseId": id.parse::<u64>().unwrap(),
                    "headSha": head,
                    "workflowName": format!("wf-{id}"),
                })
            })
            .collect();
        self.set(
            &format!("runs/{sha}.json"),
            &serde_json::to_string(&list).unwrap(),
        );
        for (id, _, conclusion, view_head) in runs {
            self.set(
                &format!("views/{id}.json"),
                &serde_json::json!({
                    "conclusion": conclusion,
                    "headSha": view_head,
                    "workflowName": format!("wf-{id}"),
                })
                .to_string(),
            );
        }
    }

    /// The stub's call log. Panics when the stub never ran, so a test that
    /// asserts an absent call cannot pass on an empty log.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.state().join("calls.log"))
            .expect("stub gh never ran: calls.log missing")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A driver that runs the given steps in YAML order, one bash process per
    /// step, exporting each step's stdout as `RECIPE_VAR_<output>` the way the
    /// runner does. With `done_fifo`, it writes `done` there at the end.
    fn driver(&self, ids: &[&str], done_fifo: Option<&Path>) -> Command {
        self.driver_env(ids, done_fifo, &[])
    }

    /// As [`Sandbox::driver`], with extra variables in every step's
    /// environment (a caller setting, as the runner exports it).
    fn driver_env(&self, ids: &[&str], done_fifo: Option<&Path>, env: &[(&str, &str)]) -> Command {
        let mut script = String::from("#!/usr/bin/env bash\nset -u\n");
        for (n, (id, output, body)) in select_steps(ids).iter().enumerate() {
            let file = self.root.join("steps").join(format!("{n:02}-{id}.sh"));
            fs::write(&file, body).unwrap();
            let out = self.root.join("out").join(id);
            script.push_str(&format!(
                "bash '{f}' > '{o}.out' 2> '{o}.err'; echo $? > '{o}.code'\n",
                f = file.display(),
                o = out.display()
            ));
            if !output.is_empty() {
                script.push_str(&format!(
                    "export RECIPE_VAR_{output}=\"$(cat '{o}.out')\"\n",
                    o = out.display()
                ));
            }
        }
        if let Some(fifo) = done_fifo {
            script.push_str(&format!("printf 'done\\n' > '{}'\n", fifo.display()));
        }
        let driver = self.root.join("steps/driver.sh");
        write_exec(&driver, &script);
        let mut command = Command::new("bash");
        command
            .arg(&driver)
            .current_dir(self.work())
            .env("REPO_PATH", self.work())
            .env("PR_NUMBER", PR)
            .env("AUTODRIVE_ROUND_LABEL", "round-1")
            .env(
                "AUTODRIVE_QA_EVIDENCE",
                self.root.join("out/qa-evidence.json"),
            )
            .stdin(Stdio::null());
        self.isolate(&mut command);
        command
            .env("REPO_PATH", self.work())
            .env("PR_NUMBER", PR)
            .env_remove(INTERVAL_VAR)
            .env_remove(INTERVAL_ALIAS);
        for (key, value) in env {
            command.env(key, value);
        }
        command
    }

    fn run(&self, ids: &[&str]) {
        self.run_env(ids, &[]);
    }

    fn run_env(&self, ids: &[&str], env: &[(&str, &str)]) {
        let output = self
            .driver_env(ids, None, env)
            .output()
            .expect("run step driver");
        assert!(
            output.status.success(),
            "driver failed: {}\n{}",
            text(&output.stdout),
            text(&output.stderr)
        );
    }

    fn result(&self, id: &str) -> StepResult {
        let read = |ext: &str| {
            fs::read_to_string(self.root.join("out").join(format!("{id}.{ext}")))
                .unwrap_or_default()
        };
        StepResult {
            code: read("code").trim().to_string(),
            stdout: read("out"),
            stderr: read("err"),
        }
    }

    /// Run one step and return its JSON, requiring exit 0.
    fn step_json(&self, id: &str) -> (Json, StepResult) {
        self.step_json_env(id, &[])
    }

    fn step_json_env(&self, id: &str, env: &[(&str, &str)]) -> (Json, StepResult) {
        self.run_env(&[id], env);
        let result = self.result(id);
        assert_eq!(
            result.code, "0",
            "step `{id}` must exit 0 with evidence\nstdout:\n{}\nstderr:\n{}",
            result.stdout, result.stderr
        );
        (result.json(id), result)
    }

    fn mkfifo(&self, name: &str) -> PathBuf {
        let path = self.state().join(name);
        let status = Command::new("mkfifo")
            .args(["-m", "600"])
            .arg(&path)
            .status()
            .expect("mkfifo");
        assert!(status.success(), "mkfifo {}", path.display());
        path
    }
}

/// A JSON field as a string, whether the step wrote a string or a bare value.
fn field(json: &Json, key: &str) -> String {
    match json.get(key) {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn run_ids(evidence: &Json) -> Vec<String> {
    evidence
        .get("runs")
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("ci_evidence must carry a `runs` array: {evidence}"))
        .iter()
        .map(|run| field(run, "id"))
        .collect()
}

/// The `--interval` value of every `gh run watch` call, `<missing>` where a
/// watch has none.
fn watch_intervals(calls: &[String]) -> Vec<String> {
    calls
        .iter()
        .filter(|c| c.starts_with("run watch"))
        .map(|c| {
            let words: Vec<&str> = c.split_whitespace().collect();
            words
                .iter()
                .position(|w| *w == "--interval")
                .and_then(|i| words.get(i + 1))
                .map_or_else(|| "<missing>".to_string(), |v| (*v).to_string())
        })
        .collect()
}

/// The setting as the runner exports it: `RECIPE_VAR_<key>` and its
/// uppercase alias, both set to `value`.
fn interval_env(value: &str) -> [(&'static str, &str); 2] {
    [(INTERVAL_VAR, value), (INTERVAL_ALIAS, value)]
}

/// One `autodrive_ci_watch_interval` note on stderr: the line names the
/// setting and the default it fell back to.
fn fallback_notes(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains(INTERVAL_KEY) && l.contains(DEFAULT_INTERVAL))
        .collect()
}

fn wait_ci(sb: &Sandbox) -> Json {
    sb.step_json(WAIT_STEP).0
}

fn sync(sb: &Sandbox) -> Json {
    sb.step_json(SYNC_STEP).0
}

// ---------------------------------------------------------------------------
// Ordering: QA waits for the head's CI run to finish
// ---------------------------------------------------------------------------

#[test]
fn qa_runs_only_after_head_ci_run_completes() {
    let sb = Sandbox::new();
    sb.merge_state("CLEAN");
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("101", &head, "success", &head)]);
    let started = sb.mkfifo("started");
    let release = sb.mkfifo("release");
    sb.set("block_watch", "");

    let ids: Vec<String> = recipe_steps().into_iter().map(|(id, _, _)| id).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let child: Child = sb
        .driver(&id_refs, Some(&started))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn step driver");

    // Blocks until the stub's watch or the driver's end writes to the FIFO.
    let first = fs::read_to_string(&started).expect("read started fifo");
    let calls_while_watching = sb.calls();
    let watch_seen = first.trim() == "started";
    if watch_seen {
        fs::write(&release, "go\n").expect("release the watch");
        let last = fs::read_to_string(&started).expect("read done");
        assert_eq!(last.trim(), "done", "driver must finish after the watch");
    }
    let output = child.wait_with_output().expect("wait driver");
    assert!(
        watch_seen,
        "no step called `gh run watch` for the run at the head before the recipe finished.\ncalls: {:#?}\ndriver stderr:\n{}",
        sb.calls(),
        text(&output.stderr)
    );
    assert!(
        !calls_while_watching.iter().any(|c| c.starts_with("qa:")),
        "QA ran while the head's CI run was still being watched: {calls_while_watching:#?}"
    );

    let calls = sb.calls();
    let pos = |pred: &dyn Fn(&str) -> bool, what: &str| {
        calls
            .iter()
            .position(|c| pred(c))
            .unwrap_or_else(|| panic!("no `{what}` call in {calls:#?}"))
    };
    let list = pos(
        &|c| c.starts_with("run list") && c.contains(&head),
        "run list --commit HEAD",
    );
    let watch = pos(
        &|c| c.starts_with("run watch") && c.contains("101"),
        "run watch 101",
    );
    let view = pos(
        &|c| c.starts_with("run view") && c.contains("101"),
        "run view 101",
    );
    let qa = pos(&|c| c.starts_with("qa:"), "qa");
    assert!(
        list < watch && watch < view && view < qa,
        "order must be run list, run watch, run view, qa: {calls:#?}"
    );
    assert_eq!(
        watch_intervals(&calls),
        [DEFAULT_INTERVAL],
        "with no caller setting, every watch polls at the default --interval {DEFAULT_INTERVAL} \
         (bb:0 rate limit): {}",
        calls[watch]
    );
    let qa_lines: Vec<&String> = calls.iter().filter(|c| c.starts_with("qa:")).collect();
    assert_eq!(qa_lines.len(), 1, "QA must run exactly once: {calls:#?}");
    assert_eq!(qa_lines[0], &format!("qa:{head}"), "QA must judge the head");
    assert!(
        !calls.iter().any(|c| c.starts_with("pr checks")),
        "CI is read from the head's runs, not polled with `gh pr checks`: {calls:#?}"
    );
    assert_eq!(
        sb.origin_ref("feat"),
        head,
        "an up-to-date round must not push"
    );

    // The ci_evidence the round reads is the wait step's, and it is GREEN.
    let wait = sb.result(WAIT_STEP);
    let evidence = wait.json(WAIT_STEP);
    assert_eq!(field(&evidence, "ci_status"), "GREEN", "{evidence}");
    assert_eq!(field(&evidence, "head_sha"), head);
    let later_ci: Vec<String> = recipe_steps()
        .into_iter()
        .skip_while(|(id, _, _)| id != WAIT_STEP)
        .skip(1)
        .filter(|(_, output, _)| output == "ci_evidence")
        .map(|(id, _, _)| id)
        .collect();
    assert!(
        later_ci.is_empty(),
        "no later step may overwrite the wait step's ci_evidence: {later_ci:?}"
    );
}

// ---------------------------------------------------------------------------
// Binding to the head
// ---------------------------------------------------------------------------

#[test]
fn ci_evidence_is_bound_to_the_head_sha() {
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(
        &head,
        &[
            ("201", &head, "success", &head),
            ("202", &head, "success", &head),
        ],
    );

    let evidence = wait_ci(&sb);
    assert_eq!(field(&evidence, "head_sha"), head, "{evidence}");
    assert_eq!(field(&evidence, "ci_status"), "GREEN", "{evidence}");
    assert_eq!(field(&evidence, "ci_total"), "2", "{evidence}");
    assert_eq!(field(&evidence, "ci_pending"), "0", "{evidence}");
    assert_eq!(field(&evidence, "ci_failing"), "0", "{evidence}");
    assert_eq!(field(&evidence, "ci_signal"), "GREEN:0/2", "{evidence}");
    let mut ids = run_ids(&evidence);
    ids.sort();
    assert_eq!(ids, ["201", "202"], "{evidence}");
    let calls = sb.calls();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("run list") && c.contains(&format!("--commit {head}"))),
        "runs must be listed for the local HEAD: {calls:#?}"
    );
    for id in ["201", "202"] {
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("run watch") && c.contains(id)),
            "run {id} must be watched: {calls:#?}"
        );
    }
}

#[test]
fn after_a_sync_push_the_wait_reads_the_new_head_not_the_old_one() {
    let sb = Sandbox::new();
    sb.merge_state("BEHIND");
    sb.active_workflow();
    let old = sb.head();
    // A green run exists only for the head before the sync.
    sb.runs(&old, &[("301", &old, "success", &old)]);
    sb.advance_main("main.txt", "main\n");

    sb.run(&[SYNC_STEP, WAIT_STEP]);
    let new = sb.head();
    assert_ne!(new, old, "a BEHIND branch must be merged forward");
    let evidence = sb.result(WAIT_STEP).json(WAIT_STEP);
    assert_eq!(field(&evidence, "head_sha"), new, "{evidence}");
    assert_eq!(field(&evidence, "ci_status"), "PENDING", "{evidence}");
    assert_eq!(
        field(&evidence, "ci_signal"),
        "no-run-for-head",
        "{evidence}"
    );
    assert!(
        !sb.calls().iter().any(|c| c.starts_with("run watch")),
        "the old head's run must not be watched: {:#?}",
        sb.calls()
    );
}

// ---------------------------------------------------------------------------
// Conclusions
// ---------------------------------------------------------------------------

/// One conclusion case: name, `(run id, conclusion)` pairs, expected
/// `ci_status` and expected `ci_failing`.
type ConclusionCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str, &'a str);

#[test]
fn green_and_red_conclusions_map_to_ci_status() {
    let cases: &[ConclusionCase] = &[
        (
            "all success",
            &[("401", "success"), ("402", "success")],
            "GREEN",
            "0",
        ),
        (
            "success and failure",
            &[("401", "success"), ("402", "failure")],
            "RED",
            "1",
        ),
        ("cancelled", &[("401", "cancelled")], "RED", "1"),
        ("timed out", &[("401", "timed_out")], "RED", "1"),
        (
            "success and skipped",
            &[("401", "success"), ("402", "skipped")],
            "GREEN",
            "0",
        ),
        ("neutral", &[("401", "neutral")], "GREEN", "0"),
        ("action required", &[("401", "action_required")], "RED", "1"),
        ("startup failure", &[("401", "startup_failure")], "RED", "1"),
        (
            "success and cancelled",
            &[("401", "success"), ("402", "cancelled")],
            "RED",
            "1",
        ),
    ];
    for (name, runs, status, failing) in cases {
        let sb = Sandbox::new();
        sb.active_workflow();
        let head = sb.head();
        let spec: Vec<(&str, &str, &str, &str)> = runs
            .iter()
            .map(|(id, c)| (*id, head.as_str(), *c, head.as_str()))
            .collect();
        sb.runs(&head, &spec);
        let evidence = wait_ci(&sb);
        assert_eq!(field(&evidence, "ci_status"), *status, "{name}: {evidence}");
        assert_eq!(
            field(&evidence, "ci_failing"),
            *failing,
            "{name}: {evidence}"
        );
        assert_eq!(field(&evidence, "ci_pending"), "0", "{name}: {evidence}");
        assert_eq!(
            field(&evidence, "ci_signal"),
            format!("{status}:{failing}/{}", runs.len()),
            "{name}: {evidence}"
        );
        let conclusions: Vec<String> = evidence["runs"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: runs array missing: {evidence}"))
            .iter()
            .map(|r| field(r, "conclusion"))
            .collect();
        assert_eq!(conclusions.len(), runs.len(), "{name}: {evidence}");
    }
}

#[test]
fn the_watch_exit_code_is_not_the_conclusion() {
    // Watch fails but the run succeeded: GREEN.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("501", &head, "success", &head)]);
    sb.set("watch_exit/501", "1");
    let evidence = wait_ci(&sb);
    assert_eq!(field(&evidence, "ci_status"), "GREEN", "{evidence}");

    // Watch succeeds but the run failed: RED.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("502", &head, "failure", &head)]);
    let evidence = wait_ci(&sb);
    assert_eq!(field(&evidence, "ci_status"), "RED", "{evidence}");
}

// ---------------------------------------------------------------------------
// Fail closed
// ---------------------------------------------------------------------------

#[test]
fn unreadable_cases_are_never_green() {
    // gh run list fails.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("601", &head, "success", &head)]);
    sb.set("run_list_fail", "");
    let evidence = wait_ci(&sb);
    assert_eq!(
        field(&evidence, "ci_status"),
        "UNREADABLE",
        "run list fails: {evidence}"
    );
    assert_eq!(
        field(&evidence, "ci_pending"),
        "0",
        "run list fails: {evidence}"
    );
    let foreign = "f".repeat(40);

    // The list says HEAD but the view reports another SHA: discarded.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("603", &head, "success", &foreign)]);
    let evidence = wait_ci(&sb);
    assert_ne!(
        field(&evidence, "ci_status"),
        "GREEN",
        "foreign view: {evidence}"
    );
    assert!(
        !run_ids(&evidence).contains(&"603".to_string()),
        "a run whose view has a foreign headSha is discarded: {evidence}"
    );

    // One run at the head cannot be viewed.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(
        &head,
        &[
            ("604", &head, "success", &head),
            ("605", &head, "success", &head),
        ],
    );
    fs::remove_file(sb.state().join("views/605.json")).unwrap();
    let evidence = wait_ci(&sb);
    assert_eq!(
        field(&evidence, "ci_status"),
        "UNREADABLE",
        "view fails: {evidence}"
    );

    // An empty or unknown conclusion.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("606", &head, "", &head)]);
    let evidence = wait_ci(&sb);
    assert_eq!(
        field(&evidence, "ci_status"),
        "UNREADABLE",
        "empty conclusion: {evidence}"
    );

    // A conclusion outside the allow-list is never green.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("607", &head, "stale", &head)]);
    let evidence = wait_ci(&sb);
    assert_ne!(
        field(&evidence, "ci_status"),
        "GREEN",
        "stale conclusion: {evidence}"
    );

    // An unreadable run outranks a pending one, and a red run outranks both:
    // one red run with one unviewable run is RED.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(
        &head,
        &[
            ("608", &head, "failure", &head),
            ("609", &head, "success", &head),
        ],
    );
    fs::remove_file(sb.state().join("views/609.json")).unwrap();
    let evidence = wait_ci(&sb);
    assert_eq!(
        field(&evidence, "ci_status"),
        "RED",
        "red and unreadable: {evidence}"
    );

    // Exactly 100 runs: the list may be truncated.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    let ids: Vec<String> = (0..100).map(|n| format!("{}", 7000 + n)).collect();
    let spec: Vec<(&str, &str, &str, &str)> = ids
        .iter()
        .map(|id| (id.as_str(), head.as_str(), "success", head.as_str()))
        .collect();
    sb.runs(&head, &spec);
    let evidence = wait_ci(&sb);
    assert_eq!(
        field(&evidence, "ci_status"),
        "UNREADABLE",
        "100 runs: {evidence}"
    );

    // No run and gh workflow list fails.
    let sb = Sandbox::new();
    sb.set("workflow_list_fail", "");
    let evidence = wait_ci(&sb);
    assert_eq!(
        field(&evidence, "ci_status"),
        "UNREADABLE",
        "workflow list fails: {evidence}"
    );
}

#[test]
fn no_workflows_gives_no_checks() {
    let sb = Sandbox::new();
    sb.set(
        "workflows.json",
        r#"[{"name":"Old","state":"disabled_manually"}]"#,
    );
    let evidence = wait_ci(&sb);
    assert_eq!(field(&evidence, "ci_status"), "NO_CHECKS", "{evidence}");
    assert_eq!(field(&evidence, "ci_signal"), "NO_CHECKS:0/0", "{evidence}");
    assert_eq!(field(&evidence, "head_sha"), sb.head(), "{evidence}");
    assert!(run_ids(&evidence).is_empty(), "{evidence}");
}

// ---------------------------------------------------------------------------
// Sync policy: push only when GitHub requires it
// ---------------------------------------------------------------------------

struct SyncCase {
    name: &'static str,
    merge_state: &'static str,
    /// Prepare the repositories; returns nothing.
    setup: fn(&Sandbox),
    reason: &'static str,
    synced: &'static str,
    conflict: &'static str,
    pushes: bool,
}

fn behind(sb: &Sandbox) {
    sb.advance_main("main.txt", "main\n");
}

fn conflicting(sb: &Sandbox) {
    sb.commit_feat("base.txt", "feat version\n");
    sb.advance_main("base.txt", "main version\n");
}

fn up_to_date(_: &Sandbox) {}

#[test]
fn sync_policy_pushes_only_when_github_requires_it() {
    let cases = [
        SyncCase {
            name: "BEHIND",
            merge_state: "BEHIND",
            setup: behind,
            reason: "behind-strict",
            synced: "true",
            conflict: "false",
            pushes: true,
        },
        SyncCase {
            name: "CLEAN while behind",
            merge_state: "CLEAN",
            setup: behind,
            reason: "not-required",
            synced: "false",
            conflict: "false",
            pushes: false,
        },
        SyncCase {
            name: "BLOCKED while behind",
            merge_state: "BLOCKED",
            setup: behind,
            reason: "not-required",
            synced: "false",
            conflict: "false",
            pushes: false,
        },
        SyncCase {
            name: "UNSTABLE while behind",
            merge_state: "UNSTABLE",
            setup: behind,
            reason: "not-required",
            synced: "false",
            conflict: "false",
            pushes: false,
        },
        SyncCase {
            name: "DIRTY with conflicts",
            merge_state: "DIRTY",
            setup: conflicting,
            reason: "conflict",
            synced: "false",
            conflict: "true",
            pushes: false,
        },
        SyncCase {
            name: "DIRTY but merges cleanly",
            merge_state: "DIRTY",
            setup: behind,
            reason: "dirty-stale",
            synced: "true",
            conflict: "false",
            pushes: true,
        },
        SyncCase {
            name: "UNKNOWN",
            merge_state: "UNKNOWN",
            setup: behind,
            reason: "state-unreadable",
            synced: "false",
            conflict: "false",
            pushes: false,
        },
        SyncCase {
            name: "empty state",
            merge_state: "",
            setup: behind,
            reason: "state-unreadable",
            synced: "false",
            conflict: "false",
            pushes: false,
        },
        SyncCase {
            name: "up to date",
            merge_state: "BEHIND",
            setup: up_to_date,
            reason: "up-to-date",
            synced: "false",
            conflict: "false",
            pushes: false,
        },
    ];
    for case in cases {
        let sb = Sandbox::new();
        sb.merge_state(case.merge_state);
        (case.setup)(&sb);
        let before_head = sb.head();
        let before_origin = sb.origin_ref("feat");
        let main_tip = sb.origin_ref("main");

        let out = sync(&sb);
        let name = case.name;
        assert_eq!(field(&out, "sync_reason"), case.reason, "{name}: {out}");
        assert_eq!(field(&out, "synced"), case.synced, "{name}: {out}");
        assert_eq!(field(&out, "conflict"), case.conflict, "{name}: {out}");
        // An empty state stops the step before the base is used.
        let want_base = if case.merge_state.is_empty() {
            ""
        } else {
            "main"
        };
        assert_eq!(field(&out, "base"), want_base, "{name}: {out}");
        assert_eq!(field(&out, "git_observed"), "true", "{name}: {out}");
        assert_eq!(field(&out, "head_sha"), sb.head(), "{name}: {out}");

        let after_origin = sb.origin_ref("feat");
        if case.pushes {
            assert_ne!(
                after_origin, before_origin,
                "{name}: must push the merge once"
            );
            assert_eq!(
                after_origin,
                sb.head(),
                "{name}: origin must hold the local head"
            );
            let contains = sb.git_output(
                &sb.work(),
                &["merge-base", "--is-ancestor", &main_tip, "HEAD"],
            );
            assert!(
                contains.status.success(),
                "{name}: head must contain origin/main"
            );
        } else {
            assert_eq!(
                after_origin, before_origin,
                "{name}: must not push (CI would restart)"
            );
            assert_eq!(sb.head(), before_head, "{name}: must not merge locally");
        }
        assert!(
            !sb.work().join(".git/MERGE_HEAD").exists(),
            "{name}: no merge may be left in progress"
        );
        let pr_reads = sb
            .calls()
            .iter()
            .filter(|c| c.starts_with("pr view"))
            .count();
        assert_eq!(
            pr_reads,
            1,
            "{name}: the PR is read exactly once: {:#?}",
            sb.calls()
        );
    }
}

#[test]
fn unreadable_pr_state_neither_merges_nor_pushes() {
    // gh pr view fails.
    let sb = Sandbox::new();
    behind(&sb);
    sb.set("pr_view_fail", "");
    let (before_head, before_origin) = (sb.head(), sb.origin_ref("feat"));
    let out = sync(&sb);
    assert_eq!(field(&out, "sync_reason"), "state-unreadable", "{out}");
    assert_eq!(field(&out, "synced"), "false", "{out}");
    assert_eq!(sb.head(), before_head, "gh failure must not merge");
    assert_eq!(
        sb.origin_ref("feat"),
        before_origin,
        "gh failure must not push"
    );

    // A base name that fails validation: no fetch, no merge, no push.
    let sb = Sandbox::new();
    behind(&sb);
    sb.merge_state("BEHIND");
    sb.set("base", "-x");
    let (before_head, before_origin) = (sb.head(), sb.origin_ref("feat"));
    let out = sync(&sb);
    assert_eq!(field(&out, "sync_reason"), "state-unreadable", "{out}");
    assert_eq!(field(&out, "synced"), "false", "{out}");
    assert_eq!(sb.head(), before_head, "an invalid base must not merge");
    assert_eq!(
        sb.origin_ref("feat"),
        before_origin,
        "an invalid base must not push"
    );
}

#[test]
fn a_merge_that_changes_nothing_is_not_pushed() {
    let sb = Sandbox::new();
    sb.merge_state("BEHIND");
    // The same change lands on both sides, so the merge leaves the tree as it was.
    sb.commit_feat("same.txt", "same\n");
    sb.advance_main("same.txt", "same\n");
    let before_origin = sb.origin_ref("feat");
    let out = sync(&sb);
    assert_eq!(field(&out, "conflict"), "false", "{out}");
    assert_eq!(
        sb.origin_ref("feat"),
        before_origin,
        "a no-op merge must not be pushed (it would restart CI for nothing): {out}"
    );
}

#[test]
fn push_failure_undoes_the_merge_and_reports_old_head_ci() {
    let sb = Sandbox::new();
    sb.merge_state("BEHIND");
    sb.active_workflow();
    let old = sb.head();
    sb.runs(&old, &[("801", &old, "success", &old)]);
    behind(&sb);
    write_exec(
        &sb.origin().join("hooks/pre-receive"),
        "#!/usr/bin/env bash\necho 'rejected by test hook' >&2\nexit 1\n",
    );
    let before_origin = sb.origin_ref("feat");

    sb.run(&[SYNC_STEP, WAIT_STEP]);
    let sync_out = sb.result(SYNC_STEP).json(SYNC_STEP);
    assert_eq!(field(&sync_out, "sync_reason"), "push-failed", "{sync_out}");
    assert_eq!(field(&sync_out, "synced"), "false", "{sync_out}");
    assert_eq!(field(&sync_out, "conflict"), "false", "{sync_out}");
    assert_eq!(
        sb.origin_ref("feat"),
        before_origin,
        "the rejected push must not move origin"
    );
    // The unpushed merge is undone, so the local head is the pushed head
    // and the next round tries the sync again.
    assert_eq!(sb.head(), old, "the rejected merge must be undone");
    assert_eq!(field(&sync_out, "head_sha"), old, "{sync_out}");
    assert!(
        !sb.work().join(".git/MERGE_HEAD").exists(),
        "no merge may be left in progress"
    );

    // CI is read for the head GitHub has, where run 801 succeeded.
    let evidence = sb.result(WAIT_STEP).json(WAIT_STEP);
    assert_eq!(field(&evidence, "head_sha"), old, "{evidence}");
    assert_eq!(field(&evidence, "ci_status"), "GREEN", "{evidence}");
}

#[test]
fn a_missing_repo_path_never_syncs_the_current_repository() {
    // REPO_PATH names no directory; the step runs from inside `work`, a
    // repository that is behind and would otherwise be merged and pushed.
    let sb = Sandbox::new();
    sb.merge_state("BEHIND");
    behind(&sb);
    let missing = sb.root.join("no-such-repo");
    let (before_head, before_origin) = (sb.head(), sb.origin_ref("feat"));
    let (before_main, before_origin_main) = (
        sb.git(&sb.seed(), &["rev-parse", "main"]),
        sb.origin_ref("main"),
    );

    let (out, result) = sb.step_json_env(SYNC_STEP, &[("REPO_PATH", missing.to_str().unwrap())]);
    assert_eq!(field(&out, "sync_reason"), "not-a-repo", "{out}");
    assert_eq!(field(&out, "synced"), "false", "{out}");
    assert_eq!(field(&out, "git_observed"), "false", "{out}");
    assert_eq!(field(&out, "head_sha"), "", "{out}");
    assert!(
        result.stderr.contains("cannot cd to REPO_PATH"),
        "{}",
        result.stderr
    );
    assert!(!missing.exists(), "the step must not create REPO_PATH");
    assert_eq!(
        sb.head(),
        before_head,
        "the current repository must not merge"
    );
    assert_eq!(
        sb.origin_ref("feat"),
        before_origin,
        "nothing may be pushed"
    );
    assert_eq!(
        sb.git(&sb.seed(), &["rev-parse", "main"]),
        before_main,
        "the other repository must not change"
    );
    assert_eq!(
        sb.origin_ref("main"),
        before_origin_main,
        "nothing may be pushed"
    );
    assert!(
        !sb.work().join(".git/MERGE_HEAD").exists(),
        "no merge may be started"
    );
    // The step stops before it reads the PR, so gh never runs.
    assert!(
        !sb.state().join("calls.log").exists(),
        "gh must not be called for a missing REPO_PATH"
    );
}

#[test]
fn a_repo_path_outside_git_reports_ci_unreadable_without_calling_gh() {
    // REPO_PATH is a plain directory: step-01b must skip visibly and report
    // UNREADABLE (never a pass) before it reads a head or calls gh (#537).
    let sb = Sandbox::new();
    let plain = sb.root.join("plain-dir");
    fs::create_dir_all(&plain).unwrap();

    let (out, result) = sb.step_json_env(WAIT_STEP, &[("REPO_PATH", plain.to_str().unwrap())]);
    assert_eq!(field(&out, "ci_status"), "UNREADABLE", "{out}");
    assert_eq!(field(&out, "ci_signal"), "no-repo", "{out}");
    assert_eq!(field(&out, "head_sha"), "", "{out}");
    assert!(
        result.stderr.contains("[skip] not a git repo"),
        "{}",
        result.stderr
    );
    assert!(
        !sb.state().join("calls.log").exists(),
        "gh must not be called outside a git repository"
    );
}

#[test]
fn unreadable_pr_state_is_not_reported_up_to_date() {
    // gh pr view fails while the branch is level with origin/main: the step
    // reports state-unreadable, never up-to-date, and does not fetch.
    let sb = Sandbox::new();
    sb.set("pr_view_fail", "");
    let fetch_head = sb.work().join(".git/FETCH_HEAD");
    assert!(!fetch_head.exists(), "the fresh clone has no FETCH_HEAD");
    let (before_head, before_origin) = (sb.head(), sb.origin_ref("feat"));

    let out = sync(&sb);
    assert_eq!(field(&out, "sync_reason"), "state-unreadable", "{out}");
    assert_eq!(field(&out, "synced"), "false", "{out}");
    assert_eq!(field(&out, "base"), "", "{out}");
    assert_eq!(sb.head(), before_head, "must not merge");
    assert_eq!(sb.origin_ref("feat"), before_origin, "must not push");
    assert!(!fetch_head.exists(), "an unreadable PR must not fetch");
    let calls = sb.calls();
    assert_eq!(calls.len(), 1, "only the PR is read: {calls:#?}");
    assert!(calls[0].starts_with("pr view"), "{calls:#?}");
    assert!(!calls.iter().any(|c| c.contains("fetch")), "{calls:#?}");
}

// ---------------------------------------------------------------------------
// PENDING: no run registered at the head yet
// ---------------------------------------------------------------------------

/// Asserts the PENDING contract for one `ci_evidence` and returns its signal.
fn assert_pending(evidence: &Json, head: &str, case: &str) -> String {
    assert_eq!(
        field(evidence, "ci_status"),
        "PENDING",
        "{case}: {evidence}"
    );
    assert_eq!(
        field(evidence, "ci_signal"),
        "no-run-for-head",
        "{case}: {evidence}"
    );
    assert_eq!(field(evidence, "head_sha"), head, "{case}: {evidence}");
    assert_eq!(field(evidence, "ci_total"), "0", "{case}: {evidence}");
    assert_eq!(field(evidence, "ci_pending"), "0", "{case}: {evidence}");
    assert!(run_ids(evidence).is_empty(), "{case}: {evidence}");
    field(evidence, "ci_signal")
}

#[test]
fn head_runs_not_registered_gives_pending() {
    // No run at the head, and the repository has an active workflow.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    let evidence = wait_ci(&sb);
    assert_pending(&evidence, &head, "no run");
    let calls = sb.calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("run watch") || c.starts_with("run view")),
        "PENDING never waits for a run to appear (no sleep, no re-list): {calls:#?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("run list")).count(),
        1,
        "the runs are listed once per round: {calls:#?}"
    );

    // Every listed run carries a foreign headSha: none is at the head yet.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    let foreign = "f".repeat(40);
    sb.runs(&head, &[("602", &foreign, "success", &foreign)]);
    let evidence = wait_ci(&sb);
    assert_pending(&evidence, &head, "foreign list");
    assert!(
        !sb.calls()
            .iter()
            .any(|c| c.contains("602") && !c.starts_with("run list")),
        "a run at another SHA is never watched or viewed: {:#?}",
        sb.calls()
    );
}

#[test]
fn two_pending_rounds_at_the_same_head_stop_as_stuck() {
    // PENDING keeps the signal `no-run-for-head`, which the no-progress rule
    // does not exempt (only a signal naming PENDING is exempt). Two rounds at
    // one head with the same open items and no run therefore stop as STUCK,
    // so a workflow that never fires cannot keep the loop running.
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    let first = assert_pending(&wait_ci(&sb), &head, "round 1");
    let second = assert_pending(&wait_ci(&sb), &head, "round 2");
    assert!(
        !first.to_ascii_uppercase().contains("PENDING"),
        "the PENDING signal must not name PENDING, or the no-progress rule never stops it: {first}"
    );

    let progress = workspace_root().join("amplifier-bundle/tools/autodrive_progress.sh");
    let state = sb.root.join("progress-state");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let tool = |args: &[&str]| {
        let mut command = Command::new("bash");
        command
            .arg(&progress)
            .args(args)
            .current_dir(sb.work())
            .stdin(Stdio::null());
        sb.isolate(&mut command)
            .output()
            .expect("run autodrive_progress.sh")
    };
    let state_s = state.to_str().unwrap();
    let record = tool(&[
        "record",
        state_s,
        "merge-ready",
        "round-1",
        &head,
        "0",
        "ci",
        &first,
    ]);
    assert!(
        record.status.success(),
        "record round 1: {}",
        text(&record.stderr)
    );
    let check = tool(&[
        "check",
        state_s,
        "merge-ready",
        "round-2",
        &head,
        "ci",
        &second,
    ]);
    assert_eq!(
        check.status.code(),
        Some(10),
        "two PENDING rounds at one head with the same open items must stop as STUCK\nstdout:\n{}\nstderr:\n{}",
        text(&check.stdout),
        text(&check.stderr)
    );
}

// ---------------------------------------------------------------------------
// Watch interval: the caller setting autodrive_ci_watch_interval
// ---------------------------------------------------------------------------

/// A sandbox with one green run at the head, waited on with `env`.
fn wait_with(env: &[(&str, &str)]) -> (Sandbox, Json, StepResult) {
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(&head, &[("911", &head, "success", &head)]);
    let (evidence, result) = sb.step_json_env(WAIT_STEP, env);
    (sb, evidence, result)
}

#[test]
fn watch_interval_defaults_to_120() {
    let (sb, evidence, _) = wait_with(&[]);
    assert_eq!(field(&evidence, "ci_status"), "GREEN", "{evidence}");
    assert_eq!(
        watch_intervals(&sb.calls()),
        [DEFAULT_INTERVAL],
        "with no setting, gh run watch polls every {DEFAULT_INTERVAL}s: {:#?}",
        sb.calls()
    );
}

#[test]
fn watch_interval_accepts_60() {
    for value in ["60", "300", "999999"] {
        let (sb, evidence, result) = wait_with(&interval_env(value));
        assert_eq!(
            field(&evidence, "ci_status"),
            "GREEN",
            "{value}: {evidence}"
        );
        assert_eq!(
            watch_intervals(&sb.calls()),
            [value],
            "a valid setting ({value}) is passed to gh run watch unchanged: {:#?}",
            sb.calls()
        );
        assert!(
            fallback_notes(&result.stderr).is_empty(),
            "a valid setting ({value}) gives no fallback note:\n{}",
            result.stderr
        );
    }
}

#[test]
fn watch_interval_falls_back_for_invalid_values() {
    let long = "9".repeat(5000);
    let cases: Vec<(&str, String)> = vec![
        ("below the minimum", "59".to_string()),
        ("not a number", "abc".to_string()),
        ("empty", String::new()),
        ("zero", "0".to_string()),
        ("negative", "-60".to_string()),
        ("leading zero (octal in bash arithmetic)", "060".to_string()),
        ("leading space", " 60".to_string()),
        ("exponent", "1e3".to_string()),
        ("seven digits", "1000000".to_string()),
        ("two lines", "60\n61".to_string()),
        ("escape sequence", "\u{1b}[31m60".to_string()),
        ("5000 digits", long.clone()),
    ];
    for (name, value) in &cases {
        let (sb, evidence, result) = wait_with(&interval_env(value));
        assert_eq!(field(&evidence, "ci_status"), "GREEN", "{name}: {evidence}");
        assert_eq!(
            watch_intervals(&sb.calls()),
            [DEFAULT_INTERVAL],
            "{name}: an invalid setting falls back to --interval {DEFAULT_INTERVAL}: {:#?}",
            sb.calls()
        );
        assert_eq!(
            fallback_notes(&result.stderr).len(),
            1,
            "{name}: one stderr note names {INTERVAL_KEY} and the default {DEFAULT_INTERVAL}:\n{}",
            result.stderr
        );
        assert!(
            !result.stderr.contains('\u{1b}'),
            "{name}: the rejected value is printed without control characters:\n{}",
            result.stderr
        );
        assert!(
            !result.stderr.contains(long.as_str()),
            "{name}: the rejected value is printed cut to a short length"
        );
    }
}

#[test]
fn watch_interval_is_never_evaluated_as_code() {
    // `@` is replaced by a marker path inside each sandbox, so a hit cannot
    // touch anything outside it.
    let templates = [
        "60;touch @",
        "a[$(touch @)]",
        "$(touch @)",
        "`touch @`",
        "x[$(touch @)]+60",
    ];
    for template in templates {
        let sb = Sandbox::new();
        sb.active_workflow();
        let head = sb.head();
        sb.runs(&head, &[("912", &head, "success", &head)]);
        let value = template.replace('@', sb.root.join("PWNED").to_str().unwrap());
        let (evidence, result) = sb.step_json_env(WAIT_STEP, &interval_env(&value));
        assert!(
            !sb.root.join("PWNED").exists() && !sb.work().join("PWNED").exists(),
            "the setting {value:?} was executed by step-01b\nstderr:\n{}",
            result.stderr
        );
        assert_eq!(
            field(&evidence, "ci_status"),
            "GREEN",
            "{value:?}: {evidence}"
        );
        assert_eq!(
            watch_intervals(&sb.calls()),
            [DEFAULT_INTERVAL],
            "{value:?} falls back to the default: {:#?}",
            sb.calls()
        );
    }
}

#[test]
fn step_01b_reads_the_setting_from_its_environment_not_a_template() {
    let (_, _, body) = select_steps(&[WAIT_STEP])
        .into_iter()
        .next()
        .expect("step-01b exists");
    assert!(
        !body.contains(&format!("{{{{{INTERVAL_KEY}}}}}")),
        "step-01b must not splice {{{{{INTERVAL_KEY}}}}} into its bash text (it is caller input)"
    );
}

// ---------------------------------------------------------------------------
// Rate limit: the gh calls of one round
// ---------------------------------------------------------------------------

#[test]
fn one_round_makes_the_minimum_gh_calls() {
    let sb = Sandbox::new();
    sb.merge_state("CLEAN");
    sb.active_workflow();
    let head = sb.head();
    let foreign = "e".repeat(40);
    sb.runs(
        &head,
        &[
            ("921", &head, "success", &head),
            ("922", &head, "success", &head),
            ("923", &foreign, "success", &foreign),
        ],
    );
    let ids: Vec<String> = recipe_steps().into_iter().map(|(id, _, _)| id).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    sb.run_env(&id_refs, &interval_env("60"));

    let calls = sb.calls();
    let gh: Vec<&String> = calls.iter().filter(|c| !c.starts_with("qa:")).collect();
    let count = |prefix: &str| gh.iter().filter(|c| c.starts_with(prefix)).count();
    assert_eq!(
        count("run list"),
        1,
        "exactly one run list per round: {gh:#?}"
    );
    for id in ["921", "922"] {
        let of = |prefix: &str| {
            gh.iter()
                .filter(|c| c.starts_with(prefix) && c.split_whitespace().any(|w| w == id))
                .count()
        };
        assert_eq!(
            of("run watch"),
            1,
            "run {id} is watched exactly once: {gh:#?}"
        );
        assert_eq!(
            of("run view"),
            1,
            "run {id} is viewed exactly once: {gh:#?}"
        );
    }
    assert!(
        !gh.iter()
            .any(|c| !c.starts_with("run list") && c.contains("923")),
        "a run at another SHA costs no call: {gh:#?}"
    );
    assert_eq!(
        count("pr checks"),
        0,
        "CI is never polled with gh pr checks: {gh:#?}"
    );
    assert_eq!(
        count("workflow list"),
        0,
        "workflows are listed only when no run exists: {gh:#?}"
    );
    assert_eq!(
        count("pr view"),
        1,
        "the PR state is read once, by step-01: {gh:#?}"
    );
    assert_eq!(
        gh.len(),
        6,
        "one round: pr view, run list, and one watch and one view per run at the head: {gh:#?}"
    );
    for interval in watch_intervals(&calls) {
        let n: u64 = interval.parse().unwrap_or_else(|_| {
            panic!("every watch passes a numeric --interval, got {interval}: {gh:#?}")
        });
        assert!(
            n >= 60,
            "every watch polls at most once a minute, got {n}: {gh:#?}"
        );
    }
    let evidence = sb.result(WAIT_STEP).json(WAIT_STEP);
    assert_eq!(field(&evidence, "ci_status"), "GREEN", "{evidence}");
    assert_eq!(field(&evidence, "ci_total"), "2", "{evidence}");
}

#[test]
fn head_runs_are_watched_one_at_a_time() {
    let sb = Sandbox::new();
    sb.active_workflow();
    let head = sb.head();
    sb.runs(
        &head,
        &[
            ("931", &head, "success", &head),
            ("932", &head, "success", &head),
        ],
    );
    let started = sb.mkfifo("started");
    let release = sb.mkfifo("release");
    sb.set("block_watch", "");
    let child: Child = sb
        .driver(&[WAIT_STEP], Some(&started))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn step driver");

    // Each blocked watch announces itself on `started`; the driver writes
    // `done` there at the end. Record what was in flight at each
    // announcement, release it, and repeat until done.
    let mut in_flight: Vec<Vec<String>> = Vec::new();
    loop {
        let msg = fs::read_to_string(&started).expect("read started fifo");
        let announced = msg.lines().filter(|l| l.trim() == "started").count();
        if announced > 0 {
            in_flight.push(sb.calls());
        }
        for _ in 0..announced {
            fs::write(&release, "go\n").expect("release a watch");
        }
        if msg.lines().any(|l| l.trim() == "done") {
            break;
        }
    }
    let output = child.wait_with_output().expect("wait driver");

    assert_eq!(
        in_flight.len(),
        2,
        "two runs at the head mean two watches, each announced alone (never in parallel): {in_flight:#?}\nstderr:\n{}",
        text(&output.stderr)
    );
    let watches = |calls: &[String]| calls.iter().filter(|c| c.starts_with("run watch")).count();
    assert_eq!(
        watches(&in_flight[0]),
        1,
        "first watch runs alone: {:#?}",
        in_flight[0]
    );
    assert_eq!(
        watches(&in_flight[1]),
        2,
        "second watch starts after the first: {:#?}",
        in_flight[1]
    );
    let first_view = in_flight[1]
        .iter()
        .position(|c| c.starts_with("run view"))
        .expect("the first run is viewed before the second watch starts");
    let second_watch = in_flight[1]
        .iter()
        .rposition(|c| c.starts_with("run watch"))
        .unwrap();
    assert!(
        first_view < second_watch,
        "order is watch, view, watch, view: {:#?}",
        in_flight[1]
    );
}

// ---------------------------------------------------------------------------
// The setting reaches step-01b from a whole-drive -c
// ---------------------------------------------------------------------------

fn recipe_yaml(name: &str) -> Value {
    let path = workspace_root().join("amplifier-bundle/recipes").join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn declared_default(recipe: &Value, key: &str) -> Option<String> {
    recipe
        .get("context")
        .and_then(|c| c.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn step<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .and_then(|s| {
            s.iter()
                .find(|st| st.get("id").and_then(Value::as_str) == Some(id))
        })
        .unwrap_or_else(|| panic!("no step `{id}`"))
}

#[test]
fn watch_interval_setting_reaches_step_01b_from_the_drive() {
    let forwarded = format!("{{{{{INTERVAL_KEY}}}}}");
    for name in [
        "auto-drive-to-merge.yaml",
        "autodrive-merge-loop.yaml",
        "autodrive-merge-round.yaml",
        RECIPE,
    ] {
        assert_eq!(
            declared_default(&recipe_yaml(name), INTERVAL_KEY).as_deref(),
            Some(DEFAULT_INTERVAL),
            "{name} declares {INTERVAL_KEY} with default \"{DEFAULT_INTERVAL}\""
        );
    }

    // auto-drive-to-merge -> merge loop sub-recipe.
    let top = recipe_yaml("auto-drive-to-merge.yaml");
    let to_loop = step(&top, "autodrive-merge-loop")
        .get("context")
        .and_then(|c| c.get(INTERVAL_KEY))
        .and_then(Value::as_str);
    assert_eq!(
        to_loop,
        Some(forwarded.as_str()),
        "auto-drive-to-merge passes {INTERVAL_KEY} to the merge loop"
    );

    // merge loop -> each round, through the loop helper's --context.
    let loop_text = fs::read_to_string(
        workspace_root().join("amplifier-bundle/recipes/autodrive-merge-loop.yaml"),
    )
    .unwrap();
    let context_arg = format!("--context \"{INTERVAL_KEY}=${{RECIPE_VAR_{INTERVAL_KEY}:-}}\"");
    assert!(
        loop_text.contains(&context_arg),
        "the merge loop passes {INTERVAL_KEY} to each round with `{context_arg}`"
    );

    // round -> evidence sub-recipe.
    let round = recipe_yaml("autodrive-merge-round.yaml");
    let to_evidence = step(&round, "merge-evidence")
        .get("context")
        .and_then(|c| c.get(INTERVAL_KEY))
        .and_then(Value::as_str);
    assert_eq!(
        to_evidence,
        Some(forwarded.as_str()),
        "the round passes {INTERVAL_KEY} to autodrive-merge-evidence"
    );
}
