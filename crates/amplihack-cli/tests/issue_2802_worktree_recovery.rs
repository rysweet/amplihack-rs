//! Behavioral contract for #2802. Recovery is performed ONLY by shipped agent
//! steps, through the installed CLI and receipt-checked pinned runner. No agent
//! executable, agent output, fetch, or missing-worktree repair is simulated.
//!
//! The live tests require an explicitly supplied API key (ISSUE_2802_OPENAI_API_KEY)
//! and real `codex` on PATH. They are ignored by ordinary cargo test; explicitly
//! selecting them fails, rather than skips, when prerequisites are unavailable.
//! The CI-only Gadugi driver selects this same suite. Never run that driver or
//! scenario on a development host. Helper refusal tests need no provider.
//!
//! This is TDD coverage: passing fixture/helper checks do not establish recovery.
//! Until live cases have run, RED and release evidence remain unestablished.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use serde_yaml::Value as Yaml;

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;

const BRANCH: &str = "feat/issue-2802";
const URL: &str = "https://github.com/fixture/project/pull/2802";

#[derive(Clone, Copy, Debug)]
enum Entry {
    Build,
    BuildResolve,
    Light,
    Merge,
    Crusty,
}

impl Entry {
    fn recipe(self) -> &'static str {
        match self {
            Self::Build | Self::BuildResolve => "autodrive-build",
            Self::Light => "auto-drive-light",
            Self::Merge => "autodrive-merge-loop",
            Self::Crusty => "autodrive-crusty-loop",
        }
    }

    fn output(self) -> &'static str {
        match self {
            Self::Build | Self::BuildResolve | Self::Light => "build_result",
            Self::Merge => "merge_loop_preflight",
            Self::Crusty => "crusty_loop_preflight",
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn copy_bundle(from: &Path, to: &Path) {
    private(to);
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_bundle(&entry.path(), &dest);
        } else if entry.file_type().unwrap().is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), dest).unwrap();
        } else {
            fs::copy(entry.path(), &dest).unwrap();
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o755;
            fs::set_permissions(dest, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}

fn show(output: &Output) -> String {
    format!(
        "exit={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    trusted: PathBuf,
    bin: PathBuf,
    caller: PathBuf,
    source: PathBuf,
    decoy: PathBuf,
    head: String,
    base: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("issue-2802-")
            .tempdir()
            .unwrap();
        private(temp.path());
        let home = temp.path().join("home");
        let trusted = temp.path().join("trusted");
        let bin = temp.path().join("bin");
        for path in [
            &home,
            &trusted,
            &bin,
            &temp.path().join("evidence"),
            &temp.path().join("tmp"),
        ] {
            private(path);
        }
        for name in [".claude", ".amplihack", ".codex", ".config", ".cache"] {
            private(&home.join(name));
        }
        copy_bundle(
            &root().join("amplifier-bundle"),
            &trusted.join("amplifier-bundle"),
        );
        let caller = temp.path().join("operator");
        let source = temp.path().join("remotes/project.git");
        let decoy = temp.path().join("operator-decoy");
        private(source.parent().unwrap());
        let seed = temp.path().join("seed");
        private(&seed);
        let mut f = Self {
            temp,
            home,
            trusted,
            bin,
            caller,
            source,
            decoy,
            head: String::new(),
            base: String::new(),
        };
        f.git(&seed, &["init", "-q", "-b", "main"]);
        fs::write(seed.join("tracked"), "base\n").unwrap();
        f.git(&seed, &["add", "tracked"]);
        f.git(&seed, &["commit", "-qm", "base"]);
        f.base = f.git(&seed, &["rev-parse", "HEAD"]);
        f.git(
            f.temp.path(),
            &[
                "clone",
                "--bare",
                "--no-local",
                seed.to_str().unwrap(),
                f.source.to_str().unwrap(),
            ],
        );
        // GitHub accepts repository URLs with and without .git. Preserve
        // both spellings for a fetch from headRepository.url or origin.
        std::os::unix::fs::symlink("project.git", f.source.with_extension("")).unwrap();
        // Clone the caller BEFORE publishing the PR head: its later private
        // clone cannot obtain that object from the operator's object store.
        f.git(
            f.temp.path(),
            &[
                "clone",
                "--single-branch",
                "--branch",
                "main",
                f.source.to_str().unwrap(),
                f.caller.to_str().unwrap(),
            ],
        );
        f.git(&f.caller, &["checkout", "--detach", &f.base]);
        f.git(
            &f.caller,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/fixture/project.git",
            ],
        );
        f.git(&seed, &["checkout", "-qb", BRANCH]);
        fs::write(seed.join("tracked"), "PR head\n").unwrap();
        f.git(&seed, &["commit", "-qam", "PR head"]);
        f.head = f.git(&seed, &["rev-parse", "HEAD"]);
        f.git(&seed, &["push", f.source.to_str().unwrap(), BRANCH]);
        f.git(
            &seed,
            &[
                "push",
                f.source.to_str().unwrap(),
                "HEAD:refs/pull/2802/head",
            ],
        );

        // A linked worktree that passes the helper but belongs to another
        // repository must never satisfy private-run ownership.
        f.git(
            f.temp.path(),
            &[
                "clone",
                "--no-local",
                f.source.to_str().unwrap(),
                f.decoy.to_str().unwrap(),
            ],
        );
        f.git(
            &f.decoy,
            &[
                "worktree",
                "add",
                "-b",
                BRANCH,
                "../operator-decoy-pr",
                &f.head,
            ],
        );
        fs::write(
            f.temp.path().join("operator-decoy-pr/KEEP"),
            "operator sentinel\n",
        )
        .unwrap();
        fs::write(f.caller.join("KEEP"), "launch sentinel\n").unwrap();
        fs::write(f.caller.join("tracked"), "unstaged launch data\n").unwrap();
        fs::write(f.caller.join("staged"), "staged launch data\n").unwrap();
        f.git(&f.caller, &["add", "staged"]);

        // These fixture-only URL rewrites keep ALL hosting Git access local,
        // including SSH alternatives. No transport security is disabled.
        for prefix in [
            "https://github.com/fixture/",
            "git@github.com:fixture/",
            "ssh://git@github.com/fixture/",
            "https://github.com/contributor/",
            "git@github.com:contributor/",
        ] {
            f.git(
                f.temp.path(),
                &[
                    "config",
                    "--global",
                    "--add",
                    &format!("url.{}/.insteadOf", f.source.parent().unwrap().display()),
                    prefix,
                ],
            );
        }
        f.metadata("OPEN", false);
        write_executable(&f.bin.join("gh"), GH, 0o700);
        write_executable(&f.bin.join("amplihack"), HELPER_AUDIT, 0o700);
        f
    }

    fn git_output(&self, dir: &Path, args: &[&str]) -> Output {
        amplihack_git::command_in(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap()
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.git_output(dir, args);
        assert!(output.status.success(), "git {args:?}: {}", show(&output));
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn metadata(&self, state: &str, fork: bool) {
        let owner = if fork { "contributor" } else { "fixture" };
        let metadata = json!({
            "number":2802,"url":URL,"state":state,"title":"Fixture PR", "body":"",
            "headRefName":BRANCH,"headRefOid":self.head,"baseRefName":"main", "baseRefOid":self.base,
            "isDraft":false,"isCrossRepository":fork,
            "headRepository":{"name":"project","nameWithOwner":format!("{owner}/project"),"url":format!("https://github.com/{owner}/project")},
            "headRepositoryOwner":{"login":owner},
            "baseRepository":{"nameWithOwner":"fixture/project","url":"https://github.com/fixture/project"},
            "head":{"ref":BRANCH,"sha":self.head,"repo":{"full_name":format!("{owner}/project"),"clone_url":format!("https://github.com/{owner}/project.git")}},
            "base":{"ref":"main","sha":self.base,"repo":{"full_name":"fixture/project","clone_url":"https://github.com/fixture/project.git"}}
        });
        fs::write(
            self.temp.path().join("pr.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = crate::isolated_amplihack::isolated(amplihack_bin());
        command
            .current_dir(&self.caller)
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("TMPDIR", self.temp.path().join("tmp"))
            .env("AMPLIHACK_HOME", &self.trusted)
            .env("AMPLIHACK_WORK_ROOT", self.temp.path().join("work"))
            .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .env("ISSUE_2802_FIXTURE", self.temp.path())
            .env("ISSUE_2802_AMPLIHACK", amplihack_bin())
            .env(
                "PATH",
                std::env::join_paths(std::iter::once(self.bin.clone()).chain(
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                ))
                .unwrap(),
            );
        command
    }

    fn helper(&self, repo: &Path, head: &str, branch: &str) -> Output {
        self.command()
            .args(["orch", "helper", "autodrive", "worktree", "--repo"])
            .arg(repo)
            .args(["--head", head, "--branch", branch])
            .output()
            .unwrap()
    }

    fn run(&self, entry: Entry, setup: &str, historical: bool) -> Observation {
        // An explicitly selected live test must never silently pass for lack
        // of provider access. Do not read/copy an operator's credential files.
        let key = std::env::var_os("ISSUE_2802_OPENAI_API_KEY")
            .filter(|key| !key.is_empty())
            .expect("INCOMPLETE #2802: supply ISSUE_2802_OPENAI_API_KEY for the real Codex adapter; no RED or release result established");
        let runner = crate::pinned_runner::pinned_runner(&root());
        let mut recipe = self.recipe(entry, historical);
        if setup == "blocked" {
            recipe["context"]["build_preflight"]["blockers"] =
                serde_yaml::to_value(["no-worktree", "implementation-failed"]).unwrap();
        }
        let path = self
            .trusted
            .join("amplifier-bundle/recipes/autodrive-issue-2802-fixture.yaml");
        fs::write(&path, serde_yaml::to_string(&recipe).unwrap()).unwrap();
        let output = self
            .command()
            .args(["recipe", "run"])
            .arg(&path)
            .args(["--format", "json", "-w"])
            .arg(&self.caller)
            .args([
                "-c",
                &format!("repo_path={}", self.caller.display()),
                "-c",
                "pr_number=2802",
                "-c",
                &format!("autodrive_pr_input={URL}"),
                "-c",
                &format!("existing_branch={BRANCH}"),
                "-c",
                &format!("autodrive_state_dir={}/state", self.temp.path().display()),
            ])
            .env("RECIPE_RUNNER_RS_PATH", runner)
            .env("OPENAI_API_KEY", &key)
            .env("CODEX_API_KEY", &key)
            .env("ISSUE_2802_SETUP", setup)
            .env("ISSUE_2802_HEAD", &self.head)
            .output()
            .unwrap();
        fs::write(
            self.temp.path().join("evidence/runner.stdout"),
            &output.stdout,
        )
        .unwrap();
        fs::write(
            self.temp.path().join("evidence/runner.stderr"),
            &output.stderr,
        )
        .unwrap();
        assert!(output.status.success(), "{entry:?}: {}", show(&output));
        let before = read_json(&self.temp.path().join("before.json"));
        let after = read_json(&self.temp.path().join("after.json"));
        let result = read_json(&self.temp.path().join("result.json"));
        assert_eq!(
            before["launch"], after["launch"],
            "launch checkout changed: {entry:?}"
        );
        assert_eq!(
            before["decoy"], after["decoy"],
            "operator decoy changed: {entry:?}"
        );
        assert_eq!(
            before["caller"], after["caller"],
            "operator checkout changed: {entry:?}"
        );
        assert_eq!(before["bare"], true, "the actual run must own a bare clone");
        assert_ne!(before["common"], before["caller_common"]);
        assert_eq!(before["launch"]["branch"], "", "launch must start detached");
        if setup.is_empty() {
            assert_eq!(
                before["has_head"], false,
                "positive fixture must require a real fetch"
            );
        }
        Observation {
            before,
            after,
            result,
            output,
        }
    }

    fn recipe(&self, entry: Entry, historical: bool) -> Yaml {
        let path = root().join(format!("amplifier-bundle/recipes/{}.yaml", entry.recipe()));
        let mut recipe: Yaml = serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let original = recipe["steps"].as_sequence().unwrap().clone();
        // Keep the production launch-bundle/authorization route. A generic
        // fixture name would omit captured policy and invalidate light's Binding.
        recipe["name"] = Yaml::String("autodrive-issue-2802-fixture".into());
        let mut steps = vec![bash("fixture-before", SNAPSHOT_BEFORE)];
        if historical {
            // A prior observation is fixture input, NOT a simulated successful
            // agent. Only the unchanged step-03 may resolve its blocker.
            recipe["context"]["build_preflight"] = serde_yaml::to_value(json!({
                "pr_found":"true","pr":"2802","pr_url":URL,"pr_state":"OPEN",
                "branch":BRANCH,"head_sha":self.head,"base_branch":"main","base_sha":self.base,
                "repo":"","reason":"earlier preflight found no linked worktree",
                "blockers":["no-worktree"],"attempts":["git worktree list: missing"]
            }))
            .unwrap();
        }
        for step in &original {
            let id = step["id"].as_str().unwrap();
            let keep = match entry {
                Entry::Build => ["step-01-build-preflight", "step-03-resolve-pr"].contains(&id),
                Entry::BuildResolve => id == "step-03-resolve-pr",
                Entry::Light => ["light-pr-input", "light-resolve-pr"].contains(&id),
                Entry::Merge => id == "step-01-merge-loop-preflight",
                Entry::Crusty => id == "step-01-crusty-loop-preflight",
            };
            if keep {
                steps.push(step.clone());
                if matches!(entry, Entry::Build) && id == "step-01-build-preflight" {
                    // Check preflight's checkout before step-03 can repair it.
                    // A final build_result alone cannot prove both entry points.
                    steps.push(bash("fixture-consume-preflight", PREFLIGHT_CONSUMER));
                }
            }
            if matches!(entry, Entry::Build) && id == "step-02-default-workflow" {
                let mut forbidden = bash(
                    id,
                    "echo 'existing OPEN PR incorrectly entered implementation' >&2; exit 1",
                );
                forbidden["condition"] = step["condition"].clone();
                steps.push(forbidden);
            }
        }
        // The real downstream condition gates a read-only consumer. Replacing
        // review/merge work here prevents unrelated actions; its output is
        // computed from the actual agent result and checked Git/helper state.
        let consumer_id = match entry {
            Entry::Merge => "step-02-merge-ready-loop",
            Entry::Crusty => "step-02-crusty-loop",
            Entry::Light => "light-eligibility",
            Entry::Build | Entry::BuildResolve => "fixture-build-consumer",
        };
        let mut consumer = bash(consumer_id, CONSUMER);
        if let Some(step) = original
            .iter()
            .find(|s| s["id"].as_str() == Some(consumer_id))
        {
            if let Some(condition) = step.get("condition") {
                consumer["condition"] = condition.clone();
            }
        } else {
            consumer["condition"] = Yaml::String(
                "build_result.should_run == 'true' and not build_result.blockers".into(),
            );
        }
        steps.push(consumer);
        steps.push(bash("fixture-after", SNAPSHOT_AFTER));
        recipe["steps"] = Yaml::Sequence(steps);
        recipe["context"]["fixture_output"] = Yaml::String(entry.output().into());
        recipe
    }

    fn assert_recovered(&self, entry: Entry, observed: &Observation) {
        let result = &observed.result;
        assert_eq!(result["should_run"], "true", "{entry:?}: {result}");
        assert_eq!(result["blockers"], json!([]), "{entry:?}: {result}");
        let repo = Path::new(result["repo"].as_str().expect("absolute verified repo"));
        assert!(repo.is_absolute());
        if observed.before["has_head"] == false {
            assert!(
                repo.starts_with(Path::new(
                    observed.before["worktree_base"].as_str().unwrap()
                )),
                "new worktrees follow the trusted private-run placement convention"
            );
        }
        assert_ne!(
            repo,
            Path::new(observed.before["launch_path"].as_str().unwrap())
        );
        let common = self.git(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        );
        assert_eq!(
            Path::new(&common).canonicalize().unwrap(),
            Path::new(observed.before["common"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            "worktree must belong to private run"
        );
        assert_eq!(self.git(repo, &["symbolic-ref", "--short", "HEAD"]), BRANCH);
        assert_eq!(self.git(repo, &["rev-parse", "HEAD"]), self.head);
        let list = self.git(repo, &["worktree", "list", "--porcelain"]);
        assert!(list.split("\n\n").any(|record| {
            record
                .lines()
                .any(|line| line == format!("worktree {}", repo.canonicalize().unwrap().display()))
        }));
        assert!(self.helper(repo, &self.head, BRANCH).status.success());
        let consumer = read_json(&self.temp.path().join("consumed.json"));
        assert_eq!(
            consumer["worktree"], result["repo"],
            "downstream must consume verified repo"
        );
        assert_eq!(consumer["verified"], true);
        if matches!(entry, Entry::Build | Entry::BuildResolve | Entry::Light) {
            let binding = &result["binding"];
            assert_eq!(binding["pr_number"], 2802);
            assert_eq!(binding["repository"], "fixture/project");
            assert_eq!(binding["head_branch"], BRANCH);
            assert_eq!(binding["head_sha"], self.head);
            assert_eq!(
                binding["run_id"],
                observed.before["authorization"]["run_id"]
            );
            assert_eq!(
                binding["policy_provenance"],
                observed.before["authorization"]["policy_provenance"]
            );
        }
        if matches!(entry, Entry::Build) {
            let preflight = read_json(&self.temp.path().join("preflight-consumed.json"));
            assert_eq!(preflight["verified"], true);
            assert_eq!(preflight["worktree"], result["repo"]);
        }
        let calls =
            fs::read_to_string(self.temp.path().join("helper-calls.jsonl")).unwrap_or_default();
        assert!(
            calls
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .any(|call| call["exit"] == 0
                    && call["result"]["verified"] == true
                    && call["result"]["worktree"] == result["repo"]),
            "agent must invoke the unchanged Worktree helper, independently of the observer"
        );
        assert!(
            !observed.output.stdout.is_empty(),
            "runner must produce an actual result"
        );
        println!(
            "PASS #2802 {entry:?}: branch={BRANCH} head={} private_common={} repo={} helper_verified=true launch_unchanged=true downstream_consumed=true",
            self.head,
            common,
            repo.display()
        );
    }
}

struct Observation {
    before: Value,
    after: Value,
    result: Value,
    output: Output,
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(
        &fs::read(path).unwrap_or_else(|error| {
            panic!("missing behavioral evidence {}: {error}", path.display())
        }),
    )
    .unwrap()
}

fn bash(id: &str, command: &str) -> Yaml {
    serde_yaml::to_value(json!({"id":id,"type":"bash","command":command})).unwrap()
}

// Only hosting reads are simulated. Git transport, CLI, runner and agent are
// real. Unknown operations, notably every hosting write, fail visibly.
const GH: &str = r#"#!/usr/bin/env bash
set -euo pipefail
root="$ISSUE_2802_FIXTURE"
printf '%s\n' "$*" >> "$root/gh.log"
kind="${1:-} ${2:-}"
shift 2 || exit 2
filter='.'
repository=fixture/project
while [ "$#" -gt 0 ]; do
  case "$1" in
    --jq|-q) filter="$2"; shift 2 ;;
    --json|--repo|-R|--hostname|--state|--head|--base|--limit) shift 2 ;;
    --*) echo "fixture gh: unsupported option $1" >&2; exit 2 ;;
    fixture/project|contributor/project) repository="$1"; shift ;;
    *) shift ;;
  esac
done
case "$kind" in
  'pr view') jq -cr "$filter" "$root/pr.json" ;;
  'pr list') jq -cr "[select(.state == \"OPEN\")] | $filter" "$root/pr.json" ;;
  'repo view') jq -cn --arg repository "$repository" '{name:"project",nameWithOwner:$repository,url:("https://github.com/"+$repository),owner:{login:($repository|split("/")[0])},defaultBranchRef:{name:"main"}}' | jq -cr "$filter" ;;
  'auth status') echo 'fixture: metadata reads available' ;;
  *) echo "fixture gh: unsupported operation $kind" >&2; exit 2 ;;
esac
"#;

const HELPER_AUDIT: &str = r#"#!/usr/bin/env bash
set -uo pipefail
if [ "${1:-} ${2:-} ${3:-} ${4:-}" != 'orch helper autodrive worktree' ]; then
  exec "$ISSUE_2802_AMPLIHACK" "$@"
fi
capture="$(mktemp "$ISSUE_2802_FIXTURE/tmp/helper.XXXXXX")" || exit 1
"$ISSUE_2802_AMPLIHACK" "$@" > "$capture"
status=$?
jq -cn --argjson status "$status" --rawfile result "$capture" \
  '{exit:$status,result:($result | fromjson? // null)}' >> "$ISSUE_2802_FIXTURE/helper-calls.jsonl"
cat "$capture"
rm -- "$capture"
exit "$status"
"#;

// Setup for existing-worktree cases is fixture preparation, never recovery.
// In missing-worktree cases the probe only reads Git state. The caller's
// snapshot is taken after the production CLI creates its real private clone.
const SNAPSHOT_BEFORE: &str = r#"
set -euo pipefail
r="$ISSUE_2802_FIXTURE"
launch="$AMPLIHACK_RUN_CHECKOUT"
common="$(git -C "$launch" rev-parse --path-format=absolute --git-common-dir)"
case "$ISSUE_2802_SETUP" in
  existing|dirty|incompatible|ahead|behind|diverged)
    git --git-dir="$common" fetch "$r/remotes/project.git" "refs/heads/feat/issue-2802"
    head="$ISSUE_2802_HEAD"
    if [ "$ISSUE_2802_SETUP" = behind ] || [ "$ISSUE_2802_SETUP" = incompatible ]; then head="$(git -C "$launch" rev-parse HEAD)"; fi
    git --git-dir="$common" worktree add -b feat/issue-2802 "$r/existing" "$head"
    case "$ISSUE_2802_SETUP" in
      dirty|incompatible) printf 'private dirty data\n' > "$r/existing/tracked"; printf 'private sentinel\n' > "$r/existing/KEEP" ;;
      ahead|diverged)
        if [ "$ISSUE_2802_SETUP" = diverged ]; then
          # Build a sibling commit from the common base before the run.
          git -C "$r/existing" checkout -b fixture-diverged "$(git -C "$launch" rev-parse HEAD)"
        fi
        printf 'unpublished\n' > "$r/existing/unpublished"
        git -C "$r/existing" add unpublished
        git -c user.name=Fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false -C "$r/existing" commit -qm unpublished
        if [ "$ISSUE_2802_SETUP" = diverged ]; then
          git -C "$r/existing" branch -f feat/issue-2802 HEAD
          git -C "$r/existing" checkout feat/issue-2802
        fi ;;
    esac
    ;;
  occupied)
    base="$(bash "$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_worktree_root.sh" base "$launch")"
    mkdir -p "$base/feat/issue-2802"
    printf 'occupied destination\n' > "$base/feat/issue-2802/KEEP"
    printf '%s\n' "$base/feat/issue-2802" > "$r/occupied-path"
    ;;
  ''|blocked) ;;
  *) echo 'unknown fixture setup' >&2; exit 2 ;;
esac
snapshot() {
  local path="$1" branch
  branch="$(git -C "$path" symbolic-ref --short -q HEAD || :)"
  jq -cn --arg head "$(git -C "$path" rev-parse HEAD)" --arg branch "$branch" \
    --arg diff "$(git -C "$path" diff --binary)" --arg index "$(git -C "$path" diff --cached --binary)" \
    --arg status "$(git -C "$path" status --porcelain=v1 --untracked-files=all)" \
    --arg keep "$(cat "$path/KEEP" 2>/dev/null || :)" \
    '{head:$head,branch:$branch,diff:$diff,index:$index,status:$status,keep:$keep}'
}
has_head=false; git -C "$launch" cat-file -e "$ISSUE_2802_HEAD^{commit}" 2>/dev/null && has_head=true
jq -cn --argjson launch "$(snapshot "$launch")" --argjson caller "$(snapshot "$r/operator")" \
  --argjson decoy "$(snapshot "$r/operator-decoy-pr")" --arg launch_path "$launch" \
  --arg common "$common" --arg caller_common "$(git -C "$r/operator" rev-parse --path-format=absolute --git-common-dir)" \
  --arg worktree_base "$(bash "$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_worktree_root.sh" base "$launch")" \
  --argjson bare "$(git --git-dir="$common" rev-parse --is-bare-repository)" --argjson has_head "$has_head" \
  --argjson authorization "$RECIPE_VAR_autodrive_authorization" \
  --arg refs "$(git --git-dir="$common" for-each-ref --format='%(refname) %(objectname)')" \
  --arg fetch_head "$(cat "$common/FETCH_HEAD" 2>/dev/null || :)" \
  '{launch:$launch,caller:$caller,decoy:$decoy,launch_path:$launch_path,common:$common,caller_common:$caller_common,worktree_base:$worktree_base,bare:$bare,has_head:$has_head,refs:$refs,fetch_head:$fetch_head,authorization:$authorization}' > "$r/before.json"
declare -f snapshot > "$r/snapshot.sh"
if [ -d "$r/existing" ]; then snapshot "$r/existing" > "$r/existing-before.json"; fi
"#;

const PREFLIGHT_CONSUMER: &str = r#"
set -euo pipefail
value="$RECIPE_VAR_build_preflight"
repo="$(printf '%s' "$value" | jq -er 'select(.pr_found == "true" and .pr_state == "OPEN" and .blockers == []) | .repo | select(length > 0)')"
"$ISSUE_2802_AMPLIHACK" orch helper autodrive worktree --repo "$repo" --head "$ISSUE_2802_HEAD" --branch feat/issue-2802 > "$ISSUE_2802_FIXTURE/preflight-consumed.json"
"#;

const CONSUMER: &str = r#"
set -euo pipefail
key="RECIPE_VAR_$RECIPE_VAR_fixture_output"
value="${!key}"
repo="$(printf '%s' "$value" | jq -er '.repo | select(length > 0)')"
head="$(git -C "$repo" rev-parse HEAD)"
branch="$(git -C "$repo" symbolic-ref --short HEAD)"
"$ISSUE_2802_AMPLIHACK" orch helper autodrive worktree --repo "$repo" --head "$head" --branch "$branch" > "$ISSUE_2802_FIXTURE/consumed.json"
"#;

const SNAPSHOT_AFTER: &str = r#"
set -euo pipefail
r="$ISSUE_2802_FIXTURE"
. "$r/snapshot.sh"
before="$(cat "$r/before.json")"
launch="$(printf '%s' "$before" | jq -r .launch_path)"
common="$(printf '%s' "$before" | jq -r .common)"
has_head=false; git -C "$launch" cat-file -e "$ISSUE_2802_HEAD^{commit}" 2>/dev/null && has_head=true
jq -cn --argjson launch "$(snapshot "$launch")" --argjson caller "$(snapshot "$r/operator")" \
  --argjson decoy "$(snapshot "$r/operator-decoy-pr")" \
  --argjson has_head "$has_head" \
  --arg refs "$(git --git-dir="$common" for-each-ref --format='%(refname) %(objectname)')" \
  --arg fetch_head "$(cat "$common/FETCH_HEAD" 2>/dev/null || :)" \
  --arg registration "$(git --git-dir="$common" worktree list --porcelain)" \
  '{launch:$launch,caller:$caller,decoy:$decoy,registration:$registration,has_head:$has_head,refs:$refs,fetch_head:$fetch_head}' > "$r/after.json"
key="RECIPE_VAR_$RECIPE_VAR_fixture_output"
printf '%s\n' "${!key}" > "$r/result.json"
if [ -d "$r/existing" ]; then snapshot "$r/existing" > "$r/existing-after.json"; fi
"#;

#[test]
fn private_run_fixture_keeps_the_pr_head_unfetched_and_launch_detached() {
    // Exercise fixture construction through the production CLI even on hosts
    // without provider access. This is infrastructure evidence, NOT recovery.
    let f = Fixture::new();
    let path = f
        .trusted
        .join("amplifier-bundle/recipes/autodrive-issue-2802-fixture.yaml");
    let recipe = json!({
        "name":"autodrive-issue-2802-fixture", "version":"1.0.0",
        "description":"Observe the private Git fixture without executing an agent",
        "context":{"fixture_output":"fixture_empty", "fixture_empty":"{}"},
        "steps":[bash("fixture-before", SNAPSHOT_BEFORE), bash("fixture-after", SNAPSHOT_AFTER)]
    });
    fs::write(&path, serde_yaml::to_string(&recipe).unwrap()).unwrap();
    let output = f
        .command()
        .args(["recipe", "run"])
        .arg(&path)
        .args(["--format", "json", "-w"])
        .arg(&f.caller)
        .env(
            "RECIPE_RUNNER_RS_PATH",
            crate::pinned_runner::pinned_runner(&root()),
        )
        .env("ISSUE_2802_SETUP", "")
        .env("ISSUE_2802_HEAD", &f.head)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", show(&output));
    let before = read_json(&f.temp.path().join("before.json"));
    let after = read_json(&f.temp.path().join("after.json"));
    assert_eq!(before["bare"], true);
    assert_eq!(before["launch"]["branch"], "");
    assert_ne!(before["common"], before["caller_common"]);
    assert_eq!(before["has_head"], false);
    assert_eq!(after["has_head"], false);
    assert_eq!(before["launch"], after["launch"]);
    assert_eq!(before["caller"], after["caller"]);
    assert_eq!(before["decoy"], after["decoy"]);
}

#[test]
fn existing_helper_refuses_launch_wrong_head_wrong_branch_and_nonroot() {
    let f = Fixture::new();
    let linked = f.temp.path().join("operator-decoy-pr");
    let good = f.helper(&linked, &f.head, BRANCH);
    assert!(good.status.success(), "{}", show(&good));
    for (repo, head, branch) in [
        (f.caller.as_path(), f.base.as_str(), "main"),
        (linked.as_path(), f.base.as_str(), BRANCH),
        (linked.as_path(), f.head.as_str(), "wrong-branch"),
        (linked.as_path(), "not-a-sha", BRANCH),
    ] {
        let output = f.helper(repo, head, branch);
        assert!(
            !output.status.success(),
            "invalid candidate was accepted: {}",
            show(&output)
        );
        assert!(
            output.stdout.is_empty(),
            "refusal cannot claim verification"
        );
    }
    let subdir = linked.join("subdir");
    private(&subdir);
    assert!(!f.helper(&subdir, &f.head, BRANCH).status.success());
}

macro_rules! live_recovery {
    ($name:ident, $entry:expr) => {
        #[test]
        #[ignore = "real Codex adapter and explicitly supplied API key; CI-only Gadugi selects these"]
        fn $name() {
            let f = Fixture::new();
            let observed = f.run($entry, "", false);
            f.assert_recovered($entry, &observed);
        }
    };
}

live_recovery!(
    live_build_preflight_recovers_and_reaches_build_result,
    Entry::Build
);
live_recovery!(live_build_resolve_recovers, Entry::BuildResolve);
live_recovery!(live_light_recovers, Entry::Light);
live_recovery!(live_merge_recovers, Entry::Merge);
live_recovery!(live_crusty_recovers, Entry::Crusty);

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_success_supersedes_historical_absence() {
    let f = Fixture::new();
    let observed = f.run(Entry::BuildResolve, "", true);
    f.assert_recovered(Entry::BuildResolve, &observed);
    let history = format!(
        "{} {}",
        observed.result["reason"], observed.result["attempts"]
    );
    assert!(
        history.contains("preflight") || history.contains("earlier"),
        "retain the earlier observation as history: {history}"
    );
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_recovery_does_not_clear_an_unrelated_implementation_blocker() {
    let f = Fixture::new();
    let observed = f.run(Entry::BuildResolve, "blocked", true);
    assert_eq!(observed.result["should_run"], "false");
    assert!(
        observed.result["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b.as_str() == Some("implementation-failed"))
    );
    assert!(!f.temp.path().join("consumed.json").exists());
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_unfetchable_head_reports_no_worktree_with_concrete_fetch_error() {
    let f = Fixture::new();
    fs::rename(&f.source, f.source.with_extension("unavailable")).unwrap();
    let observed = f.run(Entry::Crusty, "", false);
    assert_eq!(observed.result["should_run"], "false");
    assert_eq!(observed.result["skip_reason"], "no-worktree");
    let diagnostic = format!(
        "{} {}",
        observed.result["reason"], observed.result["attempts"]
    );
    assert!(
        diagnostic.contains("fetch"),
        "name the failed operation: {diagnostic}"
    );
    assert!(
        diagnostic.contains("does not appear to be a git repository"),
        "retain the actual Git diagnostic: {diagnostic}"
    );
    assert!(
        diagnostic.contains("128"),
        "retain the actual failed Git exit status: {diagnostic}"
    );
    assert!(!f.temp.path().join("consumed.json").exists());
    assert_eq!(observed.before["launch"], observed.after["launch"]);
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_valid_dirty_worktree_is_reused_without_discarding_contents() {
    let f = Fixture::new();
    let observed = f.run(Entry::Crusty, "dirty", false);
    f.assert_recovered(Entry::Crusty, &observed);
    assert_eq!(
        observed.result["repo"],
        f.temp.path().join("existing").to_str().unwrap()
    );
    assert_eq!(
        read_json(&f.temp.path().join("existing-before.json")),
        read_json(&f.temp.path().join("existing-after.json"))
    );
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_incompatible_dirty_worktree_is_preserved_and_explicitly_refused() {
    let f = Fixture::new();
    let observed = f.run(Entry::Crusty, "incompatible", false);
    assert_eq!(observed.result["should_run"], "false");
    assert_ne!(
        observed.result["skip_reason"], "no-worktree",
        "a validation conflict is not a fetch failure"
    );
    assert!(!observed.result["blockers"].as_array().unwrap().is_empty());
    assert_eq!(
        read_json(&f.temp.path().join("existing-before.json")),
        read_json(&f.temp.path().join("existing-after.json"))
    );
    assert!(!f.temp.path().join("consumed.json").exists());
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_occupied_destination_is_preserved_and_reports_creation_failure() {
    let f = Fixture::new();
    let observed = f.run(Entry::Crusty, "occupied", false);
    assert_eq!(observed.result["should_run"], "false");
    assert_eq!(observed.result["skip_reason"], "worktree-creation-failed");
    let path = fs::read_to_string(f.temp.path().join("occupied-path")).unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(path.trim()).join("KEEP")).unwrap(),
        "occupied destination\n"
    );
    assert!(!f.temp.path().join("consumed.json").exists());
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_non_open_pr_does_not_fetch_or_create_a_worktree() {
    for state in ["CLOSED", "MERGED"] {
        let f = Fixture::new();
        f.metadata(state, false);
        let observed = f.run(Entry::Crusty, "", false);
        assert_eq!(observed.result["should_run"], "false");
        assert_eq!(observed.result["skip_reason"], state.to_ascii_lowercase());
        assert!(!f.temp.path().join("consumed.json").exists());
        assert_eq!(observed.after["has_head"], false);
        assert_eq!(observed.before["refs"], observed.after["refs"]);
        assert_eq!(observed.before["fetch_head"], observed.after["fetch_head"]);
        assert!(
            !observed.after["registration"]
                .as_str()
                .unwrap()
                .contains(&format!("branch refs/heads/{BRANCH}"))
        );
    }
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_explicit_fork_pr_fetches_the_observed_source_head() {
    let f = Fixture::new();
    let fork = f.temp.path().join("fork/project.git");
    private(fork.parent().unwrap());
    fs::rename(&f.source, &fork).unwrap();
    // The target has a same-name branch at the WRONG head, and has never
    // received the fork object. Fetching that target branch cannot pass.
    f.git(
        f.temp.path(),
        &[
            "clone",
            "--bare",
            "--no-local",
            f.caller.to_str().unwrap(),
            f.source.to_str().unwrap(),
        ],
    );
    f.git(
        &f.source,
        &["update-ref", &format!("refs/heads/{BRANCH}"), &f.base],
    );
    for url in [
        "https://github.com/contributor/project.git",
        "git@github.com:contributor/project.git",
        "https://github.com/contributor/project",
        "ssh://git@github.com/contributor/project.git",
    ] {
        f.git(
            f.temp.path(),
            &[
                "config",
                "--global",
                "--add",
                &format!("url.{}.insteadOf", fork.display()),
                url,
            ],
        );
    }
    f.metadata("OPEN", true);
    let observed = f.run(Entry::BuildResolve, "", false);
    f.assert_recovered(Entry::BuildResolve, &observed);
}

#[test]
#[ignore = "real Codex adapter and explicitly supplied API key"]
fn live_crusty_preserves_ahead_and_diverged_history_and_repairs_behind() {
    for setup in ["ahead", "diverged", "behind"] {
        let f = Fixture::new();
        let observed = f.run(Entry::Crusty, setup, false);
        let before = read_json(&f.temp.path().join("existing-before.json"));
        let after = read_json(&f.temp.path().join("existing-after.json"));
        if setup == "behind" {
            f.assert_recovered(Entry::Crusty, &observed);
        } else {
            assert_eq!(before, after, "unpublished history must remain intact");
            let report = observed.result.to_string();
            assert!(
                report.contains(before["head"].as_str().unwrap()) && report.contains(&f.head),
                "record both the local and observed remote SHAs: {report}"
            );
            if setup == "diverged" {
                assert_eq!(observed.result["should_run"], "false");
                assert_eq!(observed.result["skip_reason"], "worktree-diverged");
            } else {
                assert_eq!(observed.result["should_run"], "true");
                let consumed = read_json(&f.temp.path().join("consumed.json"));
                assert_eq!(
                    consumed["head_sha"], before["head"],
                    "Crusty reviews the local ahead head"
                );
            }
        }
    }
}
