//! Fixture for #2569: the criterion-1 QA judgement.
//!
//! One isolated world per test. `HOME`, `TMPDIR`, `XDG_STATE_HOME`,
//! `AMPLIHACK_HOME` and `PATH` are private to the test, so the host's own
//! `gadugi-test`, `~/.claude` and `~/.amplihack` are never reached. No fake
//! sleeps or applies a timeout.
//!
//! The measurement (`autodrive_qa_evidence.sh`) and the recipe steps are the
//! real ones. The judgement agent is a fake that follows
//! `amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md`: it reads
//! the record, decides whether the PR is user-facing, writes and runs a
//! scenario, loops on failure and prints its judgement.

use crate::isolated_amplihack::isolated;
use crate::support::amplihack_bin::amplihack_bin;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The `name:` value of the scenario the fake agent writes.
pub const SCENARIO_NAME: &str = "shopper places an order";

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

pub fn bundle_tools() -> PathBuf {
    repo_root().join("amplifier-bundle/tools")
}

/// The `command` body of one `bash` step of one bundle recipe.
pub fn step_command(recipe: &str, id: &str) -> String {
    let path = repo_root().join("amplifier-bundle/recipes").join(recipe);
    let yaml = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {recipe}: {e}"));
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("parse recipe");
    doc.get("steps")
        .and_then(|s| s.as_sequence())
        .expect("steps")
        .iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(id))
        .unwrap_or_else(|| panic!("{recipe} has no step {id}"))
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or_else(|| panic!("{recipe} step {id} is not a bash step"))
        .to_string()
}

pub fn private_dir(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir");
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
}

pub fn write_exec(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn show(o: &Output) -> String {
    format!(
        "status={:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        text(&o.stdout),
        text(&o.stderr)
    )
}

pub fn read_json(path: &Path) -> Value {
    let body = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("parse {}: {e}\n{body}", path.display()))
}

/// The last non-empty line the step wrote to standard output, which is the
/// line the recipe runner parses when a step sets `parse_json: true`.
pub fn last_line(o: &Output) -> String {
    text(&o.stdout)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_else(|| panic!("nothing on standard output:\n{}", show(o)))
        .to_string()
}

pub fn field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("no string field {key} in {v}"))
        .to_string()
}

/// One recorded call to the fake `gadugi-test`.
#[derive(Debug, Clone)]
pub struct GadugiCall {
    pub subcommand: String,
    /// The `-d` argument, if any.
    pub dir: String,
    /// The `--scenario` argument, if any.
    pub scenario: String,
    /// How many `*.y{a,}ml` files the `-d` directory held at the time.
    pub staged_files: usize,
    /// Whether the `-d` directory resolved inside the repository.
    pub dir_in_repo: bool,
}

pub struct World {
    _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub home: PathBuf,
    pub tmpdir: PathBuf,
    pub xdg: PathBuf,
    pub amplihack_home: PathBuf,
    pub bin: PathBuf,
    pub repo: PathBuf,
    pub state: PathBuf,
    pub gadugi_log: PathBuf,
    pub agent: PathBuf,
    tool_state: PathBuf,
}

impl World {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().canonicalize().expect("canonical tempdir");
        private_dir(&base);
        let w = World {
            home: base.join("home"),
            tmpdir: base.join("tmp"),
            xdg: base.join("xdg-state"),
            amplihack_home: base.join("amplihack-home"),
            bin: base.join("bin"),
            repo: base.join("repo"),
            state: base.join("state"),
            gadugi_log: base.join("gadugi-calls.log"),
            agent: base.join("agent/criterion-1-agent.sh"),
            tool_state: base.join("tool-state"),
            base,
            _tmp: tmp,
        };
        for d in [
            &w.home,
            &w.tmpdir,
            &w.xdg,
            &w.amplihack_home,
            &w.bin,
            &w.repo,
            &w.state,
            &w.tool_state,
            &w.base.join("agent"),
        ] {
            private_dir(d);
        }
        fs::write(
            w.home.join(".gitconfig"),
            "[user]\n\tname = QA Fixture\n\temail = qa@example.test\n[init]\n\tdefaultBranch = main\n",
        )
        .expect("gitconfig");
        symlink(amplihack_bin(), w.bin.join("amplihack")).expect("amplihack on PATH");
        // A private AMPLIHACK_HOME whose bundle is the repository's own.
        fs::create_dir_all(w.amplihack_home.join("amplifier-bundle")).expect("mkdir bundle");
        for name in ["tools", "skills", "recipes"] {
            symlink(
                repo_root().join("amplifier-bundle").join(name),
                w.amplihack_home.join("amplifier-bundle").join(name),
            )
            .expect("link bundle subdirectory");
        }
        w.write_fakes();
        w.write_agent();
        w
    }

    // ── the fakes on PATH ────────────────────────────────────────────────

    fn write_fakes(&self) {
        let glog = self.gadugi_log.display();
        let st = self.tool_state.display();
        let repo = self.repo.display();
        // gadugi-test records the subcommand, the -d directory, how many
        // scenario files it held and whether it is inside the repository.
        // GADUGI_PLAN is `pass`, `fail-then-pass` or `unreachable`.
        write_exec(
            &self.bin.join("gadugi-test"),
            &format!(
                r#"#!/usr/bin/env bash
sub="${{1:-}}"; dir=""; scen=""; prev=""
for a in "$@"; do
  case "$prev" in -d) dir="$a" ;; --scenario) scen="$a" ;; esac
  prev="$a"
done
n=0
if [ -n "$dir" ] && [ -d "$dir" ]; then
  for f in "$dir"/*.yaml "$dir"/*.yml; do [ -f "$f" ] && n=$((n + 1)); done
fi
real="$(cd -- "${{dir:-.}}" 2>/dev/null && pwd -P || printf '%s' "$dir")"
inrepo=no; case "$real/" in '{repo}'/*) inrepo=yes ;; esac
printf 'sub=%s\tdir=%s\tscenario=%s\tstaged=%s\tin_repo=%s\n' \
  "$sub" "$dir" "$scen" "$n" "$inrepo" >> '{glog}'
[ "$sub" = validate ] && {{ echo "1 scenario valid"; exit 0; }}
case "${{GADUGI_PLAN:-pass}}" in
  pass) echo "1 scenario passed"; exit 0 ;;
  fail-then-pass)
    n=$(cat '{st}/gadugi-runs' 2>/dev/null || echo 0); n=$((n + 1))
    echo "$n" > '{st}/gadugi-runs'
    if [ "$n" -eq 1 ] || ! grep -Fq 'const total = 3;' '{repo}/src/app.js'; then
      echo "step 2: expected 'Order placed', saw 'TypeError: total is undefined'" >&2
      exit 1
    fi
    echo "1 scenario passed"; exit 0 ;;
  unreachable) echo "connect ECONNREFUSED 127.0.0.1:8443" >&2; exit 1 ;;
  *) echo "fake gadugi-test: unknown plan" >&2; exit 97 ;;
esac
"#
            ),
        );
        let fails = self.tool_state.join("npm-fails").display().to_string();
        write_exec(
            &self.bin.join("npm"),
            &format!(
                r#"#!/bin/sh
if [ -f '{fails}' ]; then echo "Tests: 1 failed, 2 passed, 3 total"; exit 1; fi
echo "Tests: 3 passed, 3 total"
"#
            ),
        );
        // The hosting interface answers nothing: no step under test asks it.
        write_exec(
            &self.bin.join("gh"),
            "#!/bin/sh\necho 'gh: offline fixture' >&2\nexit 1\n",
        );
    }

    /// Make the repository's declared suite command fail.
    pub fn declared_command_fails(&self) {
        fs::write(self.tool_state.join("npm-fails"), "1").expect("npm-fails");
    }

    // ── the fake agent ───────────────────────────────────────────────────

    /// The judgement step. It reads the record at `AUTODRIVE_QA_EVIDENCE`
    /// and never writes it, stages each scenario alone outside the
    /// repository, validates before running, and prints one JSON object with
    /// `qa_judgement`, `qa_judgement_notes`, `qa_handoff`, the cited files,
    /// the blockers it named and the scenario runs it made.
    fn write_agent(&self) {
        write_exec(
            &self.agent,
            &format!(
                r#"#!/usr/bin/env bash
set -uo pipefail
REC="${{AUTODRIVE_QA_EVIDENCE:?}}"
field() {{
  [ -f "$REC" ] || return 0
  jq -r --arg k "$1" 'if type == "object" then (.[$k] // "") else "" end' "$REC" 2>/dev/null
}}
jstr() {{ jq -cn --arg s "$1" '$s'; }}
BLOCKERS=(); NOTES=""
note() {{ NOTES="${{NOTES}}${{NOTES:+ }}$1"; }}
block() {{ BLOCKERS+=("$1"); note "blocker $1."; }}

DOCS="$(field docs_only)"; GSTAT="$(field gadugi_status)"; REASON="$(field qa_reason)"
PATHS="$(field qa_command_paths)"; SDIR="$(field gadugi_scenario_dir)"
[ -n "$GSTAT" ] || block "qa-measurement-unavailable"
[ "$GSTAT" = "NOT_INSTALLED" ] && block "qa-tool-not-installed:gadugi-test"
[ "$REASON" = "qa-command-failed" ] && block "qa-command-failed:suite (exit $(field qa_exit_code))"

cd -- "$REPO_PATH" || exit 1
[ -n "$SDIR" ] || SDIR="tests/agentic"
CITED=(); RUNS=(); HANDOFF="null"
if [ "$DOCS" = "true" ]; then
  JUDGEMENT="not_user_facing"
  for f in $PATHS; do CITED+=("$(jstr "$f")"); note "cited $f, prose only."; done
else
  JUDGEMENT="user_facing"
  SPATH="${{SDIR}}/checkout.yaml"; WRITTEN=false
  if [ ! -f "$SPATH" ]; then
    mkdir -p -- "$SDIR"
    printf 'name: %s\nsteps:\n  - run: node src/app.js checkout\n    expect: Order placed\n' \
      '{SCENARIO_NAME}' > "$SPATH"
    git add -- "$SPATH" && git commit -q -m "test: outside-in scenario for checkout"
    WRITTEN=true; note "wrote ${{SPATH}} and committed it."
  fi
  RESULT="fail"; ATTEMPT=0
  while [ "$ATTEMPT" -lt 3 ]; do
    ATTEMPT=$((ATTEMPT + 1))
    stage="$(mktemp -d "${{TMPDIR:?}}/scenario.XXXXXX")" || break
    cp -- "$SPATH" "$stage/"
    gadugi-test validate -d "$stage" >/dev/null 2>&1 || {{ rm -rf -- "$stage"; continue; }}
    if out="$(gadugi-test run -d "$stage" --scenario '{SCENARIO_NAME}' 2>&1)"; then
      RESULT="pass"; rm -rf -- "$stage"; note "attempt ${{ATTEMPT}}: ${{SPATH}} passed."; break
    fi
    rm -rf -- "$stage"
    case "$out" in
      *ECONNREFUSED*)
        note "attempt ${{ATTEMPT}}: failed, cause environment: the payment service is not accepting connections."
        HANDOFF="$(jq -cn '{{step_id:"step-02-qa-team-scenarios",
          failure:"the checkout scenario cannot reach the payment service",choice:"handoff",
          reason:"the service is unreachable from this host and no change to this PR can reach it",
          owner:"repository maintainer",needs:"restore the payment sandbox"}}')"
        break ;;
      *)
        note "attempt ${{ATTEMPT}}: failed, cause product; fixing the code."
        printf 'const total = 3;\nif (process.argv[2] === "checkout") console.log("Order placed: " + total);\n' > src/app.js
        git add -- src/app.js && git commit -q -m "fix: checkout prints the order confirmation" ;;
    esac
  done
  RUNS+=("$(jq -cn --arg p "$SPATH" --argjson w "$WRITTEN" --arg r "$RESULT" --argjson a "$ATTEMPT" \
    '{{path:$p,written_in_pr:$w,result:$r,attempts:$a}}')")
fi

arr() {{ local IFS=,; printf '[%s]' "$*"; }}
BJ=(); for b in ${{BLOCKERS[@]+"${{BLOCKERS[@]}}"}}; do BJ+=("$(jstr "$b")"); done
jq -cn --arg j "$JUDGEMENT" --arg n "$NOTES" --argjson h "$HANDOFF" \
  --argjson c "$(arr ${{CITED[@]+"${{CITED[@]}}"}})" --argjson b "$(arr ${{BJ[@]+"${{BJ[@]}}"}})" \
  --argjson r "$(arr ${{RUNS[@]+"${{RUNS[@]}}"}})" \
  '{{qa_judgement:$j,qa_judgement_notes:$n,qa_handoff:$h,qa_cited_files:$c,qa_blockers:$b,qa_scenario_runs:$r}}'
"#
            ),
        );
    }

    // ── environment and git ──────────────────────────────────────────────

    pub fn cmd(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = isolated(program);
        c.env("HOME", &self.home)
            .env("TMPDIR", &self.tmpdir)
            .env("XDG_STATE_HOME", &self.xdg)
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
            .stdin(Stdio::null());
        c
    }

    pub fn git(&self, args: &[&str]) -> String {
        let o = self
            .cmd("git")
            .current_dir(&self.repo)
            .args(args)
            .output()
            .expect("git");
        assert!(o.status.success(), "git {args:?}: {}", show(&o));
        text(&o.stdout).trim().to_string()
    }

    pub fn write(&self, rel: &str, body: &str) {
        let p = self.repo.join(rel);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write repo file");
    }

    pub fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// A Node repository: a root `package.json` is what makes
    /// `autodrive_gadugi_required` report `true`. The base commit is
    /// published as `refs/remotes/origin/main` and the PR branch `feature`
    /// starts from it.
    pub fn node_repo(&self, extra: &[(&str, &str)]) {
        self.git(&["init", "-q", "-b", "main"]);
        self.write(
            "package.json",
            r#"{"name":"shop","version":"1.0.0","scripts":{"test":"node test.js"}}"#,
        );
        self.write("src/app.js", "console.log(\"shop\");\n");
        self.write("docs/install.md", "# Install\n");
        self.write("README.md", "# Shop\n");
        for (rel, body) in extra {
            self.write(rel, body);
        }
        let base = self.commit("base");
        self.git(&["update-ref", "refs/remotes/origin/main", &base]);
        self.git(&["checkout", "-q", "-b", "feature"]);
    }

    /// A tools directory outside the repository holding only an
    /// `autodrive_qa_evidence.sh` with the given body.
    pub fn private_bundle(&self, name: &str, tool_body: &str) -> PathBuf {
        let tools = self.base.join(name).join("tools");
        write_exec(&tools.join("autodrive_qa_evidence.sh"), tool_body);
        tools
    }

    pub fn docs_pr(&self) -> String {
        self.write("docs/install.md", "# Install\n\nRun `npm install`.\n");
        self.commit("docs: explain install")
    }

    pub fn feature_pr(&self) -> String {
        self.write(
            "src/app.js",
            "console.log(\"shop\");\nif (process.argv[2] === \"checkout\") { console.log(\"Order placed: \" + total); }\n",
        );
        self.commit("feat: checkout command")
    }

    /// The files this PR changed against its base, from git itself.
    pub fn diff_files(&self) -> BTreeSet<String> {
        self.git(&["diff", "--name-only", "main...HEAD"])
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }

    pub fn evidence_path(&self) -> PathBuf {
        self.state.join("qa-evidence.json")
    }

    // ── the real pieces ──────────────────────────────────────────────────

    /// The measurement as `step-02a-qa-inputs` runs it, asserted to exit 0,
    /// with the record it wrote returned.
    pub fn record(&self) -> Value {
        let o = self
            .cmd("bash")
            .arg(bundle_tools().join("autodrive_qa_evidence.sh"))
            .current_dir(&self.repo)
            .env("REPO_PATH", &self.repo)
            .env("AUTODRIVE_QA_EVIDENCE", self.evidence_path())
            .env("AUTODRIVE_QA_BASE", "main")
            .env("AUTODRIVE_ROUND_LABEL", "round-1")
            .output()
            .expect("run autodrive_qa_evidence.sh");
        assert_eq!(o.status.code(), Some(0), "{}", show(&o));
        read_json(&self.evidence_path())
    }

    /// A bash step of a bundle recipe, run as the recipe runner runs it.
    pub fn run_step(&self, recipe: &str, id: &str, extra: &[(&str, &str)]) -> Output {
        self.cmd("bash")
            .arg("-c")
            .arg(step_command(recipe, id))
            .current_dir(&self.repo)
            .env("RECIPE_VAR_repo_path", &self.repo)
            .envs(extra.iter().copied())
            .output()
            .expect("run recipe step")
    }

    /// Every recorded `gadugi-test` call, in order.
    pub fn gadugi_calls(&self) -> Vec<GadugiCall> {
        fs::read_to_string(&self.gadugi_log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let get = |k: &str| -> String {
                    line.split('\t')
                        .find_map(|f| f.strip_prefix(&format!("{k}=")))
                        .unwrap_or_default()
                        .to_string()
                };
                GadugiCall {
                    subcommand: get("sub"),
                    dir: get("dir"),
                    scenario: get("scenario"),
                    staged_files: get("staged").parse().unwrap_or(0),
                    dir_in_repo: get("in_repo") == "yes",
                }
            })
            .collect()
    }
}
