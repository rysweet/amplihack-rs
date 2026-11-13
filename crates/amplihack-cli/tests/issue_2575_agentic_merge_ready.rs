//! Issue #2575: merge readiness is an agentic step written up in the PR.
//!
//! Contract: docs/reference/autodrive-merge-ready-round.md.
//!
//! - The shape of the merge-ready write-up never decides anything. An agent
//!   answers one word (`ready`, `not-ready`, `hand-off`);
//!   `autodrive_round_word` maps it to the loop's one-field round file and an
//!   unclear word is another round, never `STUCK`.
//! - `autodrive_round_file` stores the write-up (from standard input), the
//!   verdict and the head captured before the assessment, privately and never
//!   through a planted symlink.
//! - The merge depends only on the captured authorisation, the assessed head,
//!   GitHub's checks and GitHub accepting `--match-head-commit`
//!   (`autodrive_merge`, the finalize `merge` step).
//! - The merge loop step maps the driver's exit itself: 79 and the usage limit
//!   stay terminal, and the loop result is read from the driver's own result
//!   file, never from text an agent printed.
//!
//! Every process runs under `isolated("bash")` with HOME, AMPLIHACK_HOME (a
//! copied bundle), CLAUDE_CONFIG_DIR and the XDG directories in a temporary
//! folder, and with fake `gh` and `amplihack` first on PATH. Nothing reaches
//! GitHub or an agent, and nothing under the real `~/.amplihack` or
//! `~/.claude` is named. tests/gadugi/* is not run here.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    jq_available, private_dir, private_file, recipe, repo_root, show, stdout,
};
use crate::support::exec_stub::write_executable;

const PR: &str = "2575";
const SHA_A: &str = concat!("15a7f6f47c1e2d0b9a8f", "7e6d5c4b3a2918070605");
const SHA_B: &str = concat!("aaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaaaaaaa");
const RUN_ID: &str = "a3f9c2e1-7b44-4d0e-9a51-2c6f0e8d1b37";
const PROVENANCE: &str = concat!(
    "trusted-launch-bundle:sha256:",
    "0123456789abcdef0123456789abcdef",
    "0123456789abcdef0123456789abcdef"
);

// ---------------------------------------------------------------------------
// The three write-up shapes reported on #2575. Each round's prose is fed to
// the drive unchanged; none of them may stop it.
// ---------------------------------------------------------------------------

/// An attempt with keys the old schema did not know.
const FIXTURE_UNKNOWN_KEYS: &str = "Merge-ready assessment of PR #2575 (FIXTURE-UNKNOWN-KEYS).\n\
```json\n\
{\"criteria\":{\"checks\":{\"status\":\"met\",\"attempts\":[{\"operation\":\"gh pr checks\",\"outcome\":\"ok\",\"cause\":\"none\",\"recovery\":\"none\",\"endpoint\":\"/repos/o/r/pulls/2575\",\"response_status\":200}]}}}\n\
```\n\
Blocker: none. Outcome: ready.\n";

/// A NODE_OPTIONS notice printed before the JSON.
const FIXTURE_NOTICE_FIRST: &str = "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: amplihack config set node.heap_consent <true|false>\n\
{\"criteria\":{\"qa\":{\"status\":\"met\"}},\"marker\":\"FIXTURE-NOTICE-FIRST\"}\n\
Outcome: ready.\n";

/// A null where the old schema required text.
const FIXTURE_NULL_CAUSE: &str = "{\"criteria\":{\"approvals\":{\"status\":\"met\",\"attempts\":[{\"operation\":\"gh pr view\",\"outcome\":\"ok\",\"cause\":null,\"recovery\":null}]}},\"marker\":\"FIXTURE-NULL-CAUSE\"}\n\
The PR is ready to merge.\n";

const FIXTURES: [(&str, &str); 3] = [
    (
        "unknown keys endpoint and response_status",
        FIXTURE_UNKNOWN_KEYS,
    ),
    ("NODE_OPTIONS notice before the JSON", FIXTURE_NOTICE_FIRST),
    ("null cause", FIXTURE_NULL_CAUSE),
];

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// A fake `gh` driven by files in `$FAKE_STATE`:
/// `state` (OPEN), `title`, `body`, `server_head`, `required` and `all`
/// (pass | fail | pending | none), `merge_rc`, `rc79_on` (view | checks |
/// merge). Every call is logged to `gh.log`, one argv per line joined by
/// U+001F; the merge body read from stdin goes to `merge-body`.
const FAKE_GH: &str = r#"#!/usr/bin/env bash
set -u
S="$FAKE_STATE"
( IFS=$'\037'; printf '%s\n' "$*" ) >> "$S/gh.log"
[ -f "$S/rc79_on" ] && [ "$(cat "$S/rc79_on")" = "${2:-}" ] && { echo "HTTP 401: Bad credentials (fixture terminal refusal)" >&2; exit 79; }
opt() { local want="$1" prev="" a; shift; for a in "$@"; do [ "$prev" = "$want" ] && { printf '%s' "$a"; return 0; }; prev="$a"; done; return 1; }
case "${1:-} ${2:-}" in
  "pr view")
    obj="$(jq -cn --arg state "$(cat "$S/state" 2>/dev/null || echo OPEN)" \
      --arg title "$(cat "$S/title" 2>/dev/null || echo 'Retire the parsed record')" \
      --rawfile body "$S/body" --arg head "$(cat "$S/server_head")" \
      '{number:2575,state:$state,title:$title,body:$body,headRefOid:$head,isDraft:false}')"
    if q="$(opt --jq "$@")" || q="$(opt -q "$@")"; then printf '%s' "$obj" | jq -r "$q"
    elif f="$(opt --json "$@")"; then printf '%s' "$obj" | jq -c --arg f "$f" '. as $o | reduce ($f | split(","))[] as $k ({}; .[$k] = $o[$k])'
    else printf '%s\n' "$obj"; fi
    exit 0 ;;
  "pr checks")
    req=0; for a in "$@"; do [ "$a" = --required ] && req=1; done
    if [ "$req" = 1 ]; then mode="$(cat "$S/required" 2>/dev/null || echo pass)"; else mode="$(cat "$S/all" 2>/dev/null || echo pass)"; fi
    case "$mode" in
      none)
        if [ "$req" = 1 ]; then echo "no required checks reported on the 'feat/issue-2575' branch" >&2
        else echo "no checks reported on the 'feat/issue-2575' branch" >&2; fi
        exit 1 ;;
      pass) b=pass; rc=0 ;;
      fail) b=fail; rc=1 ;;
      *) b=pending; rc=8 ;;
    esac
    if opt --json "$@" >/dev/null; then jq -cn --arg b "$b" '[{name:"ci",bucket:$b,state:($b | ascii_upcase)}]'
    else printf 'ci\t%s\t1m\thttps://example.invalid/ci\n' "$b"; fi
    exit "$rc" ;;
  "pr merge")
    cat > "$S/merge-body"
    sha="$(opt --match-head-commit "$@")" || sha=""
    if [ "$sha" != "$(cat "$S/server_head")" ]; then
      echo "GraphQL: Head branch was modified. Review and try the merge again. (mergePullRequest)" >&2; exit 1
    fi
    [ -f "$S/merge_rc" ] && { echo "fixture: merge refused by branch protection" >&2; exit "$(cat "$S/merge_rc")"; }
    printf MERGED > "$S/state"
    exit 0 ;;
esac
exit 0
"#;

/// A fake `amplihack`: `orch helper autodrive authorization` answers from
/// `auth_out` / `auth_rc`; `extract-json` / `extract-field` are jq-based.
/// Every call is logged to `amplihack.log`.
const FAKE_AMPLIHACK: &str = r#"#!/usr/bin/env bash
set -u
S="$FAKE_STATE"
( IFS=$'\037'; printf '%s\n' "$*" ) >> "$S/amplihack.log"
case "${1:-} ${2:-} ${3:-} ${4:-}" in
  "orch helper autodrive authorization")
    [ -f "$S/auth_out" ] && cat "$S/auth_out"
    exit "$(cat "$S/auth_rc" 2>/dev/null || echo 0)" ;;
esac
if [ "${1:-} ${2:-}" = "orch helper" ]; then
  in="$(cat)"
  if [ "${3:-}" = "extract-json" ]; then
    f=""; while [ $# -gt 0 ]; do [ "$1" = "--require-field" ] && f="${2:-}"; shift; done
    printf '%s\n' "$in" | jq -R -c --arg f "$f" 'fromjson? | select(type == "object" and has($f))' 2>/dev/null | tail -n 1
    exit 0
  fi
  if [ "${3:-}" = "extract-field" ]; then
    f=""; d=""
    while [ $# -gt 0 ]; do case "$1" in --field) f="${2:-}" ;; --default) d="${2:-}" ;; esac; shift; done
    v=""
    [ -n "$in" ] && v="$(printf '%s' "$in" | jq -r --arg f "$f" 'if .[$f] == null then empty else .[$f] end | if type == "string" then . else tojson end' 2>/dev/null)"
    if [ -n "$v" ]; then printf '%s' "$v"; else printf '%s' "$d"; fi
    exit 0
  fi
fi
exit 0
"#;

/// Replaces `autodrive_loop.sh` in the copied bundle for the loop-step
/// tests: writes `loop_result` (when present) to the driver's own result file
/// under umask 077, prints `loop_stdout` (text an agent might have printed)
/// and exits `loop_rc`.
const FAKE_LOOP_DRIVER: &str = r#"#!/usr/bin/env bash
S="$FAKE_STATE"; dir=""; prev=""
for a in "$@"; do [ "$prev" = --state-dir ] && dir="$a"; prev="$a"; done
printf '%s\n' "$@" > "$S/loop-argv"
if [ -f "$S/loop_result" ]; then
  ( umask 077; jq -cn --arg r "$(cat "$S/loop_result")" \
      '{loop:"merge-ready",loop_result:$r,round_label:"round-2",reason:"fixture stop"}' > "$dir/merge-ready-result.json" )
fi
[ -f "$S/loop_stdout" ] && cat "$S/loop_stdout"
exit "$(cat "$S/loop_rc" 2>/dev/null || echo 0)"
"#;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    ah: PathBuf,
    bin: PathBuf,
    fake: PathBuf,
    state: PathBuf,
    repo: PathBuf,
}

fn run_ok(cmd: &mut Command, what: &str) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(out.status.success(), "{what}: {}", show(&out));
}

impl Fx {
    fn new() -> Option<Self> {
        if !jq_available() {
            eprintln!("SKIP: jq is not installed; the fakes and tools need it");
            return None;
        }
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let p = |n: &str| root.join(n);
        let (home, ah, bin, fake, state, repo) = (
            p("home"),
            p("amplihack-home"),
            p("bin"),
            p("fake-state"),
            p("state"),
            p("repo"),
        );
        for d in [&home, &bin, &fake, &repo, &ah.join("amplifier-bundle")] {
            std::fs::create_dir_all(d).unwrap();
        }
        for d in [
            "claude-config",
            "xdg-state",
            "xdg-config",
            "xdg-cache",
            "xdg-data",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        private_dir(&state);
        // AMPLIHACK_HOME is a copy of the launch bundle's tools and recipes.
        for sub in ["tools", "recipes"] {
            run_ok(
                Command::new("cp")
                    .arg("-R")
                    .arg(repo_root().join("amplifier-bundle").join(sub))
                    .arg(ah.join("amplifier-bundle")),
                "copy bundle",
            );
        }
        run_ok(
            Command::new("chmod").arg("-R").arg("go-w").arg(&ah),
            "chmod bundle",
        );
        write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        write_executable(&bin.join("amplihack"), FAKE_AMPLIHACK, 0o755);
        std::fs::write(
            fake.join("body"),
            "Description text.\n\n## Merge readiness\nVerdict: MERGE_READY\n",
        )
        .unwrap();
        std::fs::write(fake.join("server_head"), SHA_A).unwrap();
        Some(Fx {
            _tmp: tmp,
            root,
            home,
            ah,
            bin,
            fake,
            state,
            repo,
        })
    }

    fn set(&self, name: &str, value: &str) {
        std::fs::write(self.fake.join(name), value).unwrap();
    }

    fn tools(&self) -> PathBuf {
        self.ah.join("amplifier-bundle/tools")
    }

    /// An isolated bash with every home-like directory in the fixture.
    fn bash(&self) -> Command {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        );
        let mut c = crate::isolated_amplihack::isolated("bash");
        c.env("HOME", &self.home)
            .env("PATH", path)
            .env("AMPLIHACK_HOME", &self.ah)
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude-config"))
            .env("XDG_STATE_HOME", self.root.join("xdg-state"))
            .env("XDG_CONFIG_HOME", self.root.join("xdg-config"))
            .env("XDG_CACHE_HOME", self.root.join("xdg-cache"))
            .env("XDG_DATA_HOME", self.root.join("xdg-data"))
            .env("FAKE_STATE", &self.fake)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .current_dir(&self.root);
        c
    }

    /// Runs `script` with the round-word tool (and the state helpers it
    /// relies on) sourced from AMPLIHACK_HOME, feeding `stdin`.
    fn round_tool(&self, script: &str, args: &[&str], stdin: &[u8]) -> Output {
        let full = format!(
            "set -u\n. \"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_state.sh\" || exit 90\n\
             . \"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_round_word.sh\" || exit 91\n{script}"
        );
        let mut c = self.bash();
        c.arg("-c").arg(full).arg("bash").args(args);
        feed(c, stdin)
    }

    fn merge_tool(&self, pr: &str, sha: &str) -> Output {
        let mut c = self.bash();
        c.arg("-c")
            .arg(
                "set -u\n. \"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge.sh\" || exit 90\n\
                 autodrive_merge \"$1\" \"$2\"",
            )
            .arg("bash")
            .args([pr, sha]);
        feed(c, b"")
    }

    fn log(&self, name: &str) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.fake.join(name))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\u{1f}').map(str::to_string).collect())
            .collect()
    }

    fn gh_calls(&self) -> Vec<Vec<String>> {
        self.log("gh.log")
    }

    fn merge_calls(&self) -> Vec<Vec<String>> {
        self.gh_calls()
            .into_iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some("pr")
                    && c.get(1).map(String::as_str) == Some("merge")
            })
            .collect()
    }

    /// A git repository at `self.repo` with one commit; returns its HEAD.
    fn git_repo(&self) -> String {
        let git = |args: &[&str]| crate::support::quick_change_repo::git(&self.repo, args);
        git(&["init", "-q"]);
        std::fs::write(self.repo.join("README.md"), "fixture\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-q", "-m", "init"]);
        git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn commit(&self, message: &str) -> String {
        let git = |args: &[&str]| crate::support::quick_change_repo::git(&self.repo, args);
        git(&["commit", "-q", "--allow-empty", "-m", message]);
        git(&["rev-parse", "HEAD"]).trim().to_string()
    }
}

fn feed(mut c: Command, stdin: &[u8]) -> Output {
    use std::io::Write;
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("spawn bash");
    child.stdin.take().unwrap().write_all(stdin).ok();
    child.wait_with_output().expect("wait for bash")
}

/// The last stdout line that is a JSON object holding `field`.
fn last_json(o: &Output, field: &str) -> Value {
    stdout(o)
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v.get(field).is_some())
        .unwrap_or_else(|| panic!("no JSON line with `{field}`:\n{}", show(o)))
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

fn step_by_prefix(recipe_name: &str, prefix: &str) -> serde_yaml::Value {
    recipe(recipe_name)["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| {
            s["id"]
                .as_str()
                .is_some_and(|id| id == prefix || id.starts_with(&format!("{prefix}-")))
        })
        .cloned()
        .unwrap_or_else(|| panic!("{recipe_name}: no step `{prefix}`"))
}

fn command_of(recipe_name: &str, prefix: &str) -> String {
    step_by_prefix(recipe_name, prefix)["command"]
        .as_str()
        .unwrap_or_else(|| panic!("{recipe_name}: step `{prefix}` is not a bash step"))
        .to_string()
}

// ===========================================================================
// autodrive_round_word: one word in, one verdict out
// ===========================================================================

fn word(fx: &Fx, input: &str) -> Value {
    let out = fx.round_tool("autodrive_round_word \"$1\"", &[input], b"");
    assert_eq!(out.status.code(), Some(0), "{input:?}: {}", show(&out));
    last_json(&out, "merge_ready_verdict")
}

#[test]
fn round_word_maps_the_three_outcomes() {
    let Some(fx) = Fx::new() else { return };
    for (input, w, verdict) in [
        ("ready", "ready", "MERGE_READY"),
        ("Ready.", "ready", "MERGE_READY"),
        ("  READY\n", "ready", "MERGE_READY"),
        ("not-ready", "not-ready", "NOT_MERGE_READY"),
        ("Not-Ready", "not-ready", "NOT_MERGE_READY"),
        ("hand-off", "hand-off", "HANDOFF"),
        ("`hand-off`", "hand-off", "HANDOFF"),
    ] {
        assert_eq!(
            word(&fx, input),
            json!({"word": w, "merge_ready_verdict": verdict}),
            "{input:?}"
        );
    }
}

#[test]
fn unclear_word_is_not_ready() {
    let Some(fx) = Fx::new() else { return };
    for input in [
        "",
        "   ",
        "Probably ready",
        "ready, but CI is pending",
        "readynot-ready",
        "handoff",
        "Hand off",
        "MERGE_READY",
        "{\"merge_ready_verdict\":\"MERGE_READY\"}",
        "yes",
        "réady",
    ] {
        assert_eq!(
            word(&fx, input),
            json!({"word": "not-ready", "merge_ready_verdict": "NOT_MERGE_READY"}),
            "an unclear answer is another round, never a merge or STUCK: {input:?}"
        );
    }
}

// ===========================================================================
// autodrive_round_file: private, bounded, never through a symlink
// ===========================================================================

fn round_file(fx: &Fx, label: &str, verdict: &str, head: &str, writeup: &[u8]) -> Output {
    fx.round_tool(
        "autodrive_round_file \"$1\" \"$2\" \"$3\" \"$4\"",
        &[fx.state.to_str().unwrap(), label, verdict, head],
        writeup,
    )
}

#[test]
fn round_file_stores_any_prose_shape_verbatim() {
    let Some(fx) = Fx::new() else { return };
    for (i, (name, prose)) in FIXTURES.iter().enumerate() {
        for verdict in ["MERGE_READY", "NOT_MERGE_READY", "HANDOFF"] {
            let label = format!("round-{i}-{}", verdict.to_lowercase());
            let out = round_file(&fx, &label, verdict, SHA_A, prose.as_bytes());
            assert_eq!(out.status.code(), Some(0), "{name}: {}", show(&out));
            let base = fx.state.join(format!("merge-ready-{label}"));
            let record: Value = serde_json::from_str(
                &std::fs::read_to_string(base.with_extension("json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                record,
                json!({"merge_ready_verdict": verdict}),
                "the round file holds only the verdict ({name})"
            );
            assert_eq!(
                std::fs::read_to_string(base.with_extension("writeup")).unwrap(),
                *prose,
                "{name}"
            );
            assert_eq!(
                std::fs::read_to_string(base.with_extension("head"))
                    .unwrap()
                    .trim(),
                SHA_A
            );
            for ext in ["json", "writeup", "head"] {
                assert_eq!(mode(&base.with_extension(ext)), 0o600, "{label}.{ext}");
            }
        }
    }
}

#[test]
fn round_file_accepts_a_sha256_head() {
    let Some(fx) = Fx::new() else { return };
    let head = "b".repeat(64);
    let out = round_file(&fx, "round-1", "MERGE_READY", &head, b"ready\n");
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
}

#[test]
fn round_word_rejects_label_dot_dot() {
    let Some(fx) = Fx::new() else { return };
    let long = "x".repeat(65);
    for label in ["..", ".", "../escape", "a/b", "", "round 1", long.as_str()] {
        let out = round_file(&fx, label, "MERGE_READY", SHA_A, b"text");
        assert_eq!(
            out.status.code(),
            Some(2),
            "label {label:?}: {}",
            show(&out)
        );
    }
    assert!(
        !fx.root.join("merge-ready-..json").exists() && !fx.root.join("escape.json").exists(),
        "nothing is written outside the state directory"
    );
}

#[test]
fn round_file_rejects_bad_verdicts_heads_and_state_dirs() {
    let Some(fx) = Fx::new() else { return };
    for verdict in ["", "READY", "merge_ready", "DONE", "MERGE_READY "] {
        let out = round_file(&fx, "round-1", verdict, SHA_A, b"text");
        assert_eq!(
            out.status.code(),
            Some(2),
            "verdict {verdict:?}: {}",
            show(&out)
        );
    }
    let upper = SHA_A.to_uppercase();
    let (longer, non_hex) = (format!("{SHA_A}0"), "g".repeat(40));
    for head in [
        "",
        "abc",
        &SHA_A[..39],
        upper.as_str(),
        longer.as_str(),
        non_hex.as_str(),
    ] {
        let out = round_file(&fx, "round-1", "MERGE_READY", head, b"text");
        assert_eq!(out.status.code(), Some(2), "head {head:?}: {}", show(&out));
    }
    // A state directory that is a symlink fails autodrive_private_dir.
    let link = fx.root.join("state-link");
    symlink(&fx.state, &link).unwrap();
    let out = fx.round_tool(
        "autodrive_round_file \"$1\" round-1 MERGE_READY \"$2\"",
        &[link.to_str().unwrap(), SHA_A],
        b"text",
    );
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    assert!(!fx.state.join("merge-ready-round-1.json").exists());
}

#[test]
fn writeup_not_written_through_symlink() {
    let Some(fx) = Fx::new() else { return };
    let victim = fx.root.join("victim.txt");
    std::fs::write(&victim, "untouched\n").unwrap();
    for ext in ["json", "writeup", "head"] {
        symlink(&victim, fx.state.join(format!("merge-ready-round-1.{ext}"))).unwrap();
    }
    let out = round_file(
        &fx,
        "round-1",
        "NOT_MERGE_READY",
        SHA_A,
        b"planted write-up\n",
    );
    // Whether the tool sets the planted links aside or replaces them, the
    // file they point at is never written.
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched\n");
    // A planted link is replaced, not followed, and the round is still stored.
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    {
        let w = fx.state.join("merge-ready-round-1.writeup");
        assert!(
            !std::fs::symlink_metadata(&w)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&w).unwrap(), "planted write-up\n");
    }
}

#[test]
fn round_file_cuts_the_writeup_at_256_kib() {
    let Some(fx) = Fx::new() else { return };
    let big = "write-up line about criterion seven\n".repeat(300 * 1024 / 36 + 1);
    assert!(big.len() > 256 * 1024);
    let out = round_file(&fx, "round-1", "NOT_MERGE_READY", SHA_A, big.as_bytes());
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let stored = std::fs::read(fx.state.join("merge-ready-round-1.writeup")).unwrap();
    assert!(
        stored.len() <= 256 * 1024 + 512,
        "cut at 256 KiB plus one truncation line, got {} bytes",
        stored.len()
    );
    assert!(stored.len() < big.len());
    assert!(
        big.as_bytes().starts_with(&stored[..4096]),
        "the kept part is a prefix"
    );
}

#[test]
fn round_file_never_takes_the_writeup_from_argv() {
    let Some(fx) = Fx::new() else { return };
    // A fifth argument is not the write-up: the body comes from stdin.
    let out = fx.round_tool(
        "autodrive_round_file \"$1\" round-1 MERGE_READY \"$2\" 'ARGV-WRITEUP'",
        &[fx.state.to_str().unwrap(), SHA_A],
        b"stdin write-up\n",
    );
    if out.status.code() == Some(0) {
        assert_eq!(
            std::fs::read_to_string(fx.state.join("merge-ready-round-1.writeup")).unwrap(),
            "stdin write-up\n"
        );
    } else {
        assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    }
}

// ===========================================================================
// autodrive_merge: GitHub's conditions only
// ===========================================================================

fn assert_no_merge_attempt(fx: &Fx) {
    assert!(
        fx.merge_calls().is_empty(),
        "no merge may be attempted: {:?}",
        fx.gh_calls()
    );
}

#[test]
fn merge_goes_ahead_on_matching_head_and_green_checks() {
    let Some(fx) = Fx::new() else { return };
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = last_json(&out, "result");
    assert_eq!(v["result"], json!("merged"));
    assert_eq!(v["pr"], json!(2575));
    assert_eq!(v["expected_head"], json!(SHA_A));
    let merges = fx.merge_calls();
    assert_eq!(merges.len(), 1, "one merge request, no retries: {merges:?}");
    let m = &merges[0];
    let after = |flag: &str| {
        m.iter()
            .position(|a| a == flag)
            .and_then(|i| m.get(i + 1))
            .cloned()
    };
    assert!(m.contains(&PR.to_string()), "{m:?}");
    assert!(m.contains(&"--squash".to_string()), "{m:?}");
    assert_eq!(after("--match-head-commit").as_deref(), Some(SHA_A));
    assert_eq!(
        after("--body-file").as_deref(),
        Some("-"),
        "body on stdin (#2056)"
    );
    assert_eq!(
        after("--subject").as_deref(),
        Some("Retire the parsed record (#2575)"),
        "subject from the trusted merge-message helper (#2056)"
    );
    let body = std::fs::read_to_string(fx.fake.join("merge-body")).unwrap();
    assert!(body.starts_with("Description text."), "{body:?}");
    assert!(
        !body.contains("Merge readiness"),
        "the report is cut: {body:?}"
    );
    // The PR state is read again after the merge.
    let last = fx.gh_calls().last().cloned().unwrap();
    assert_eq!(&last[..2], ["pr", "view"], "{last:?}");
}

#[test]
fn moved_head_is_refused_then_handoff() {
    let Some(fx) = Fx::new() else { return };
    fx.set("server_head", SHA_B);
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(4), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("refused"));
    assert_eq!(fx.merge_calls().len(), 1, "GitHub decides; no retry");
}

#[test]
fn github_refusal_for_protection_or_reviews_is_refused() {
    let Some(fx) = Fx::new() else { return };
    fx.set("merge_rc", "1");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(4), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("refused"));
}

#[test]
fn failing_checks_prevent_merge_attempt() {
    let Some(fx) = Fx::new() else { return };
    for checks in ["fail", "pending"] {
        fx.set("required", checks);
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(3), "{checks}: {}", show(&out));
        assert_eq!(
            last_json(&out, "result")["result"],
            json!("checks-not-green"),
            "{checks}"
        );
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn no_required_checks_falls_back_to_all_checks() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "none");
    fx.set("all", "pass");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("merged"));
    let checks: Vec<_> = fx
        .gh_calls()
        .into_iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("checks"))
        .collect();
    assert!(
        checks
            .iter()
            .any(|c| !c.contains(&"--required".to_string())),
        "the fallback reads all checks: {checks:?}"
    );
}

#[test]
fn failing_checks_after_the_fallback_prevent_merge_attempt() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "none");
    fx.set("all", "fail");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    assert_no_merge_attempt(&fx);
}

#[test]
fn no_checks_at_all_is_handed_off() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "none");
    fx.set("all", "none");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    assert_eq!(
        last_json(&out, "result")["result"],
        json!("checks-not-green")
    );
    assert_no_merge_attempt(&fx);
}

#[test]
fn a_pr_that_is_not_open_is_not_merged() {
    let Some(fx) = Fx::new() else { return };
    for state in ["CLOSED", "MERGED"] {
        fx.set("state", state);
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(5), "{state}: {}", show(&out));
        assert_eq!(last_json(&out, "result")["result"], json!("not-open"));
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn gh_exit_79_passes_through_merge_tool() {
    let Some(fx) = Fx::new() else { return };
    for at in ["view", "checks", "merge"] {
        let _ = std::fs::remove_file(fx.fake.join("gh.log"));
        fx.set("state", "OPEN");
        fx.set("rc79_on", at);
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(79), "79 at {at}: {}", show(&out));
        let calls = fx.gh_calls();
        let idx = calls
            .iter()
            .position(|c| c.get(1).map(String::as_str) == Some(at))
            .expect("the refusing call was made");
        assert_eq!(
            idx + 1,
            calls.len(),
            "nothing runs after a 79 at {at}: {calls:?}"
        );
    }
}

#[test]
fn merge_tool_rejects_bad_arguments_without_calling_gh() {
    let Some(fx) = Fx::new() else { return };
    let upper = SHA_A.to_uppercase();
    for (pr, sha) in [
        ("", SHA_A),
        ("0", SHA_A),
        ("-1", SHA_A),
        ("12a", SHA_A),
        ("--repo=x", SHA_A),
        (PR, ""),
        (PR, "abc"),
        (PR, upper.as_str()),
        (PR, "HEAD"),
    ] {
        let out = fx.merge_tool(pr, sha);
        assert_eq!(out.status.code(), Some(2), "{pr:?} {sha:?}: {}", show(&out));
    }
    assert!(fx.gh_calls().is_empty(), "{:?}", fx.gh_calls());
}

#[test]
fn merge_tool_refuses_a_symlinked_merge_message_helper() {
    let Some(fx) = Fx::new() else { return };
    let helper = fx.tools().join("autodrive_merge_message.sh");
    let moved = fx.root.join("elsewhere-merge-message.sh");
    std::fs::rename(&helper, &moved).unwrap();
    symlink(&moved, &helper).unwrap();
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    assert_no_merge_attempt(&fx);
}

// ===========================================================================
// autodrive-finalize `merge` step: authorisation, assessed head, then GitHub
// ===========================================================================

struct FinalizeRun {
    no_merge: &'static str,
    dry_run: &'static str,
}

const AUTHORISED: FinalizeRun = FinalizeRun {
    no_merge: "false",
    dry_run: "false",
};

fn finalize_fixture() -> Option<(Fx, String)> {
    let fx = Fx::new()?;
    let head = fx.git_repo();
    fx.set("server_head", &head);
    private_file(
        &fx.state.join("merge-ready-latest.head"),
        &format!("{head}\n"),
    );
    fx.set(
        "auth_out",
        &json!({"allows_merge": true, "run_id": RUN_ID}).to_string(),
    );
    Some((fx, head))
}

fn finalize_merge(fx: &Fx, run: &FinalizeRun) -> Output {
    let command = command_of("autodrive-finalize", "merge");
    let invocation = fx.state.join("invocation.json");
    let mut c = fx.bash();
    c.arg("-c")
        .arg(command)
        .env("RECIPE_VAR_pr_number", PR)
        .env("RECIPE_VAR_repo_path", &fx.repo)
        .env("REPO_PATH", &fx.repo)
        .env("RECIPE_VAR_autodrive_state_dir", &fx.state)
        .env("RECIPE_VAR_autodrive_invocation_file", &invocation)
        .env("RECIPE_VAR_no_merge", run.no_merge)
        .env("RECIPE_VAR_autodrive_dry_run", run.dry_run)
        .current_dir(&fx.repo)
        .stdin(Stdio::null());
    c.output().expect("run finalize merge step")
}

fn finalize_result(out: &Output) -> Value {
    last_json(out, "result")
}

#[test]
fn finalize_merges_on_captured_authorisation_assessed_head_and_green_checks() {
    let Some((fx, head)) = finalize_fixture() else {
        return;
    };
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("merged"), "{v}");
    assert_eq!(v["expected_head"], json!(head));
    assert_eq!(v["handoff"], json!("false"));
    let m = &fx.merge_calls()[0];
    let i = m.iter().position(|a| a == "--match-head-commit").unwrap();
    assert_eq!(m[i + 1], head);
    let auth_calls = fx.log("amplihack.log");
    let auth = auth_calls
        .iter()
        .find(|c| {
            c.join(" ")
                .starts_with("orch helper autodrive authorization")
        })
        .expect("the captured invocation is consulted");
    assert!(
        auth.contains(&fx.state.join("invocation.json").display().to_string()),
        "{auth:?}"
    );
    assert!(auth.contains(&fx.state.display().to_string()), "{auth:?}");
}

#[test]
fn merge_refused_without_captured_authorization() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    // The helper fails on a merge run (bad invocation, wrong directory):
    // exit 2, no stdout. The PR is handed off, not reported as refused.
    std::fs::remove_file(fx.fake.join("auth_out")).unwrap();
    fx.set("auth_rc", "2");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("authorization-unreadable"), "{v}");
    assert_eq!(v["handoff"], json!("true"), "{v}");
    assert_eq!(
        v["reason"],
        json!("the captured invocation could not be read (helper exit 2)")
    );
    assert_no_merge_attempt(&fx);

    // Exit 0 with no object holding a boolean allows_merge is also unreadable.
    fx.set("auth_rc", "0");
    fx.set("auth_out", r#"{"allows_merge": "yes"}"#);
    let out = finalize_merge(&fx, &AUTHORISED);
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("authorization-unreadable"), "{v}");
    assert_eq!(v["handoff"], json!("true"), "{v}");
    assert_no_merge_attempt(&fx);

    // The captured invocation does not allow a merge.
    fx.set(
        "auth_out",
        &json!({"allows_merge": false, "run_id": RUN_ID}).to_string(),
    );
    let out = finalize_merge(&fx, &AUTHORISED);
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("not-authorised"), "{v}");
    assert_eq!(v["handoff"], json!("false"), "{v}");
    assert_no_merge_attempt(&fx);
}

#[test]
fn verify_only_run_is_not_handed_off_when_the_helper_would_fail() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    std::fs::remove_file(fx.fake.join("auth_out")).unwrap();
    fx.set("auth_rc", "2");
    let out = finalize_merge(
        &fx,
        &FinalizeRun {
            no_merge: "true",
            dry_run: "false",
        },
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("not-authorised"), "{v}");
    assert_eq!(
        v["handoff"],
        json!("false"),
        "a verify-only run is not a hand-off"
    );
    assert_no_merge_attempt(&fx);
}

#[test]
fn recipe_context_can_narrow_but_never_grant_merge_permission() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    for run in [
        FinalizeRun {
            no_merge: "true",
            dry_run: "false",
        },
        FinalizeRun {
            no_merge: "false",
            dry_run: "true",
        },
        FinalizeRun {
            no_merge: "",
            dry_run: "false",
        },
        FinalizeRun {
            no_merge: "FALSE",
            dry_run: "false",
        },
        FinalizeRun {
            no_merge: "false",
            dry_run: "",
        },
    ] {
        let out = finalize_merge(&fx, &run);
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        assert_eq!(
            finalize_result(&out)["result"],
            json!("not-authorised"),
            "no_merge={:?} dry_run={:?}",
            run.no_merge,
            run.dry_run
        );
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn head_moved_after_assessment_is_handed_off() {
    let Some((fx, assessed)) = finalize_fixture() else {
        return;
    };
    let moved = fx.commit("committed after the assessment");
    assert_ne!(moved, assessed);
    fx.set("server_head", &moved);
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("refused"), "{v}");
    assert_eq!(v["handoff"], json!("true"));
    assert!(
        fx.gh_calls().is_empty(),
        "a moved head is refused without calling gh: {:?}",
        fx.gh_calls()
    );
}

#[test]
fn missing_or_malformed_assessed_head_never_merges() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    let head_file = fx.state.join("merge-ready-latest.head");
    for content in [None, Some("not-a-sha\n"), Some("\n")] {
        let _ = std::fs::remove_file(&head_file);
        if let Some(c) = content {
            private_file(&head_file, c);
        }
        let out = finalize_merge(&fx, &AUTHORISED);
        let v = finalize_result(&out);
        assert_ne!(v["result"], json!("merged"), "{content:?}: {v}");
        assert_eq!(v["handoff"], json!("true"), "{content:?}: {v}");
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn failing_checks_at_finalisation_are_handed_off() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    fx.set("required", "fail");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("checks-not-green"));
    assert_eq!(v["handoff"], json!("true"));
    assert_no_merge_attempt(&fx);
}

#[test]
fn github_refusal_at_finalisation_is_handed_off() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    fx.set("server_head", SHA_B); // moved on the server only
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("refused"));
    assert_eq!(v["handoff"], json!("true"));
}

#[test]
fn gh_exit_79_passes_through_merge_step() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    fx.set("rc79_on", "checks");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(79), "{}", show(&out));
    assert_no_merge_attempt(&fx);
}

// ===========================================================================
// `amplihack orch helper autodrive authorization` (real binary)
// ===========================================================================

fn write_invocation(dir: &Path, state_dir: &Path, no_merge: bool, dry_run: bool) -> PathBuf {
    private_dir(dir);
    let path = dir.join("invocation.json");
    let action = if no_merge || dry_run {
        "verify_readiness"
    } else {
        "merge"
    };
    private_file(
        &path,
        &json!({
            "schema_version": 1,
            "selected_pr": 2575,
            "selected_url": null,
            "selected_branch": null,
            "policy_provenance": PROVENANCE,
            "run_id": RUN_ID,
            "no_merge": no_merge,
            "dry_run": dry_run,
            "requested_action": action,
            "state_dir": state_dir,
        })
        .to_string(),
    );
    path
}

fn authorization_helper(fx: &Fx, invocation: &Path, directory: &Path) -> Output {
    let mut c = crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
    c.args([
        "orch",
        "helper",
        "autodrive",
        "authorization",
        "--invocation",
    ])
    .arg(invocation)
    .arg("--directory")
    .arg(directory)
    .env("HOME", &fx.home)
    .env("CLAUDE_CONFIG_DIR", fx.root.join("claude-config"))
    .env("AMPLIHACK_HOME", &fx.ah)
    .current_dir(&fx.root)
    .stdin(Stdio::null());
    c.output().expect("run amplihack authorization helper")
}

#[test]
fn authorization_helper_reports_captured_merge_permission() {
    let Some(fx) = Fx::new() else { return };
    for (no_merge, dry_run, allows) in [
        (false, false, true),
        (true, false, false),
        (false, true, false),
    ] {
        let inv = write_invocation(
            &fx.root.join(format!("inv-{no_merge}-{dry_run}")),
            &fx.state,
            no_merge,
            dry_run,
        );
        let out = authorization_helper(&fx, &inv, &fx.state);
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        assert_eq!(
            last_json(&out, "allows_merge"),
            json!({"allows_merge": allows, "run_id": RUN_ID})
        );
    }
}

#[test]
fn authorization_helper_refuses_unbound_or_unreadable_invocations() {
    let Some(fx) = Fx::new() else { return };
    let inv = write_invocation(&fx.root.join("inv"), &fx.state, false, false);
    let other = fx.root.join("other-state");
    private_dir(&other);
    for (invocation, directory) in [
        (inv.clone(), other.clone()),
        (fx.root.join("inv/missing.json"), fx.state.clone()),
    ] {
        let out = authorization_helper(&fx, &invocation, &directory);
        assert_eq!(out.status.code(), Some(2), "{}", show(&out));
        assert!(
            stdout(&out).trim().is_empty(),
            "nothing on stdout: {}",
            show(&out)
        );
    }
    // A group-readable invocation is not private evidence.
    std::fs::set_permissions(&inv, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = authorization_helper(&fx, &inv, &fx.state);
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
}

// ===========================================================================
// autodrive-merge-loop step-02 and step-02a: exit mapping and loop result
// ===========================================================================

struct LoopCase {
    rc: &'static str,
    result: Option<&'static str>,
    printed: Option<String>,
}

fn loop_step(fx: &Fx, case: &LoopCase) -> (Output, Option<Output>) {
    write_executable(
        &fx.tools().join("autodrive_loop.sh"),
        FAKE_LOOP_DRIVER,
        0o755,
    );
    fx.set("loop_rc", case.rc);
    let _ = std::fs::remove_file(fx.fake.join("loop_result"));
    let _ = std::fs::remove_file(fx.fake.join("loop_stdout"));
    let _ = std::fs::remove_file(fx.state.join("merge-ready-result.json"));
    if let Some(r) = case.result {
        fx.set("loop_result", r);
    }
    if let Some(p) = &case.printed {
        fx.set("loop_stdout", p);
    }
    let review = fx.root.join("review");
    std::fs::create_dir_all(&review).unwrap();
    let preflight = json!({
        "should_run": "true", "pr": PR, "pr_url": "", "pr_state": "OPEN",
        "branch": "feat/issue-2575", "head_sha": SHA_A, "base_branch": "main",
        "base_sha": SHA_B, "state_dir": fx.state, "repo": fx.repo,
        "review_dir": review, "reason": "fixture", "blockers": [], "attempts": []
    })
    .to_string();
    let env = |c: &mut Command| {
        c.env("RECIPE_VAR_merge_loop_preflight", &preflight)
            .env("RECIPE_VAR_task_description", "issue 2575 fixture task")
            .env("RECIPE_VAR_autodrive_state_dir", &fx.state)
            .env("RECIPE_VAR_pr_number", PR)
            .env("RECIPE_VAR_no_merge", "false")
            .env("RECIPE_VAR_autodrive_dry_run", "false")
            .env(
                "RECIPE_VAR_autodrive_invocation_file",
                fx.state.join("invocation.json"),
            )
            .env("REPO_PATH", &fx.repo)
            .current_dir(&fx.repo)
            .stdin(Stdio::null());
    };
    let mut c = fx.bash();
    c.arg("-c")
        .arg(command_of("autodrive-merge-loop", "step-02"));
    env(&mut c);
    let out = c.output().expect("run merge loop step-02");
    if out.status.code() != Some(0) {
        return (out, None);
    }
    let mut c = fx.bash();
    c.arg("-c")
        .arg(command_of("autodrive-merge-loop", "step-02a"))
        .env("RECIPE_VAR_merge_loop_result", stdout(&out));
    env(&mut c);
    let classified = c.output().expect("run merge loop step-02a");
    (out, Some(classified))
}

fn loop_fixture() -> Option<Fx> {
    let fx = Fx::new()?;
    fx.git_repo();
    Some(fx)
}

#[test]
fn step02_keeps_refusal_and_usage_limit_exits() {
    let Some(fx) = loop_fixture() else { return };
    for (rc, result, expected) in [
        ("79", None, 79),
        ("79", Some("DONE"), 79),
        ("1", Some("USAGE_LIMIT"), 1),
        ("75", Some("ORPHANED"), 75),
        ("2", None, 2),
        ("3", None, 3),
    ] {
        let (out, _) = loop_step(
            &fx,
            &LoopCase {
                rc,
                result,
                printed: None,
            },
        );
        assert_eq!(
            out.status.code(),
            Some(expected),
            "driver {rc}/{result:?}: {}",
            show(&out)
        );
    }
}

#[test]
fn step02_turns_every_other_loop_ending_into_a_handoff() {
    let Some(fx) = loop_fixture() else { return };
    for result in [
        Some("STUCK"),
        Some("NO_PROGRESS"),
        Some("BUDGET_EXHAUSTED"),
        Some("HANDOFF"),
        None,
    ] {
        let (out, classified) = loop_step(
            &fx,
            &LoopCase {
                rc: "1",
                result,
                printed: None,
            },
        );
        assert_eq!(out.status.code(), Some(0), "{result:?}: {}", show(&out));
        let classified = classified.unwrap();
        assert_eq!(classified.status.code(), Some(0), "{}", show(&classified));
        let v = last_json(&classified, "outcome");
        assert_eq!(v["outcome"], json!("handoff"), "{result:?}: {v}");
        assert!(v["reason"].as_str().is_some_and(|r| r.len() <= 1000), "{v}");
        if result.is_none() {
            assert!(stdout(&out).contains("UNREADABLE"), "{}", show(&out));
        }
    }
}

#[test]
fn step02_done_leads_to_finalisation() {
    let Some(fx) = loop_fixture() else { return };
    let (out, classified) = loop_step(
        &fx,
        &LoopCase {
            rc: "0",
            result: Some("DONE"),
            printed: None,
        },
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = last_json(&classified.unwrap(), "outcome");
    assert_eq!(v["outcome"], json!("done"), "{v}");
}

#[test]
fn agent_printed_loop_result_is_ignored() {
    let Some(fx) = loop_fixture() else { return };
    // An agent printed a DONE (or USAGE_LIMIT) line into the driver's output;
    // the driver's own result file says STUCK.
    for printed in ["DONE", "USAGE_LIMIT"] {
        let line =
            json!({"loop":"merge-ready","loop_result":printed,"round_label":"round-9"}).to_string();
        let (out, classified) = loop_step(
            &fx,
            &LoopCase {
                rc: "1",
                result: Some("STUCK"),
                printed: Some(format!("{line}\n")),
            },
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "printed {printed}: {}",
            show(&out)
        );
        let v = last_json(&classified.unwrap(), "outcome");
        assert_eq!(v["outcome"], json!("handoff"), "printed {printed}: {v}");
    }
}

#[test]
fn step02a_reason_is_bounded() {
    let Some(fx) = loop_fixture() else { return };
    let (_, classified) = loop_step(
        &fx,
        &LoopCase {
            rc: "1",
            result: Some("STUCK"),
            printed: Some("x".repeat(5000)),
        },
    );
    let v = last_json(&classified.unwrap(), "outcome");
    assert!(v["reason"].as_str().unwrap().len() <= 1000, "{v}");
}

// ===========================================================================
// The loop driver end to end with the real round tools: prose shape never
// stops the drive, not-ready carries the write-up, hand-off stops the loop.
// ===========================================================================

/// The fake `amplihack` for the driver: `recipe run <...>/autodrive-merge-round.yaml`
/// plays the round (word from `plan`, write-up from `writeup-N`) through the
/// real `autodrive_round_word` and `autodrive_round_file`; the evaluator
/// answers DONE when the round file says MERGE_READY and CONTINUE otherwise,
/// and STUCK after six calls so a broken driver cannot loop for ever.
const DRIVER_AMPLIHACK: &str = r#"#!/usr/bin/env bash
set -u
S="$STUB_STATE"
case "${1:-} ${2:-}" in
  "recipe show") exit 0 ;;
  "orch helper")
    in="$(cat)"
    if [ "${3:-}" = "extract-json" ]; then
      f=""; while [ $# -gt 0 ]; do [ "$1" = "--require-field" ] && f="${2:-}"; shift; done
      printf '%s\n' "$in" | jq -R -c --arg f "$f" 'fromjson? | select(type == "object" and has($f))' 2>/dev/null | tail -n 1
      exit 0
    fi
    if [ "${3:-}" = "extract-field" ]; then
      f=""; d=""
      while [ $# -gt 0 ]; do case "$1" in --field) f="${2:-}" ;; --default) d="${2:-}" ;; esac; shift; done
      v=""
      [ -n "$in" ] && v="$(printf '%s' "$in" | jq -r --arg f "$f" 'if .[$f] == null then empty else .[$f] end | if type == "string" then . else tojson end' 2>/dev/null)"
      if [ -n "$v" ]; then printf '%s' "$v"; else printf '%s' "$d"; fi
      exit 0
    fi
    printf '%s\n' "$*" >> "$S/helper-calls"
    exit 0 ;;
  "recipe run") ;;
  *) exit 0 ;;
esac
name="$(basename -- "${3:-}" .yaml)"
label=""; wd=""; prev=""
for a in "$@"; do
  case "$prev:$a" in
    -c:autodrive_round_label=*) label="${a#autodrive_round_label=}" ;;
    -c:loop_round_label=*) label="${a#loop_round_label=}" ;;
    --context-from-file:loop_round_label=*) label="$(cat -- "${a#*=}")" ;;
  esac
  [ "$prev" = "--working-dir" ] && wd="$a"
  prev=$a
done
if [ "$name" = "loop-health-evaluator" ]; then
  printf '%s\n' "$label" >> "$S/evaluator-calls"
  if [ "$(wc -l < "$S/evaluator-calls")" -gt 6 ]; then echo "LOOP_HEALTH: STUCK"; exit 1; fi
  v="$(jq -r '.merge_ready_verdict // empty' "$STUB_STATE_DIR/merge-ready-$label.json" 2>/dev/null)"
  verdict=CONTINUE; [ "$v" = MERGE_READY ] && verdict=DONE
  printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: %s\n' "$verdict"
  exit 0
fi
[ "$name" = "autodrive-merge-round" ] || exit 0
printf '%s\n' "$label" >> "$S/round-calls"
n="${label#round-}"
printf '%s\0' "$@" > "$S/round-argv-$n"
prev=""
for a in "$@"; do
  if [ "$prev" = "--context-from-file" ]; then cp -- "${a#*=}" "$S/ctx-$n-${a%%=*}" 2>/dev/null || :; fi
  prev=$a
done
line="$(sed -n "${n}p" "$S/plan")"; [ -n "$line" ] || line="$(tail -n 1 "$S/plan")"
IFS='|' read -r word commit <<<"$line"
w="$S/writeup-$n"; [ -f "$w" ] || w="$S/writeup"
head="$(git -C "$wd" rev-parse HEAD)"                       # step-01a
. "$TOOLS/autodrive_state.sh" || exit 2
. "$TOOLS/autodrive_round_word.sh" || exit 2
v="$(autodrive_round_word "$word" | jq -r .merge_ready_verdict)" || exit 2   # step-03, step-05a
if [ "$commit" = 1 ]; then                                    # step-04
  t="$(git -C "$wd" write-tree)"
  c="$(git -C "$wd" -c user.name=t -c user.email=t@t commit-tree "$t" -p HEAD -m "repair $label")"
  git -C "$wd" update-ref HEAD "$c"
fi
autodrive_round_file "$STUB_STATE_DIR" "$label" "$v" "$head" < "$w" || exit 2   # step-05
echo "round ran"
exit 0
"#;

const DRIVER_GH: &str = r#"#!/usr/bin/env bash
set -u
if [ "${1:-} ${2:-}" = 'pr view' ]; then git -C "$STUB_REPO" rev-parse HEAD; exit $?; fi
printf '%s\n' "$*" >> "$STUB_STATE/gh-calls"
exit 0
"#;

struct Drive {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    stub: PathBuf,
    state: PathBuf,
    repo: PathBuf,
    main: PathBuf,
    task: PathBuf,
}

impl Drive {
    fn new() -> Option<Self> {
        if !jq_available() {
            eprintln!("SKIP: jq is not installed");
            return None;
        }
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let p = |n: &str| root.join(n);
        let (bin, stub, state, repo, main, task) = (
            p("bin"),
            p("stub"),
            p("state"),
            p("repo"),
            p("main"),
            p("task-in"),
        );
        for d in [&bin, &stub, &main, &p("home"), &p("claude-config")] {
            std::fs::create_dir_all(d).unwrap();
        }
        private_dir(&state);
        std::fs::write(&task, "issue 2575 fixture task\n").unwrap();
        let git = |args: &[&str]| crate::support::quick_change_repo::git(&main, args);
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "--detach", repo.to_str().unwrap()]);
        write_executable(&bin.join("amplihack"), DRIVER_AMPLIHACK, 0o755);
        write_executable(&bin.join("gh"), DRIVER_GH, 0o755);
        Some(Drive {
            _tmp: tmp,
            root,
            bin,
            stub,
            state,
            repo,
            main,
            task,
        })
    }

    /// One line per round: `WORD|COMMIT`; the last line repeats.
    fn plan(&self, lines: &[&str]) {
        std::fs::write(self.stub.join("plan"), lines.join("\n") + "\n").unwrap();
    }

    fn writeup(&self, round: usize, text: &str) {
        std::fs::write(self.stub.join(format!("writeup-{round}")), text).unwrap();
    }

    fn run(&self) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        );
        let tools = repo_root().join("amplifier-bundle/tools");
        let mut c = crate::isolated_amplihack::isolated("bash");
        c.arg(tools.join("autodrive_loop.sh"))
            .args([
                "--loop-name",
                "merge-ready",
                "--round-recipe",
                "autodrive-merge-round",
            ])
            .args([
                "--clean-token",
                "MERGE_READY",
                "--verdict-field",
                "merge_ready_verdict",
            ])
            .arg("--repo")
            .arg(&self.repo)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--task-file")
            .arg(&self.task)
            .args([
                "--context",
                "repo_path=.",
                "--context",
                &format!("pr_number={PR}"),
            ])
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude-config"))
            .env("TOOLS", &tools)
            .env("STUB_STATE", &self.stub)
            .env("STUB_STATE_DIR", &self.state)
            .env("STUB_REPO", &self.repo)
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&self.main)
            .stdin(Stdio::null());
        c.output().expect("spawn autodrive_loop.sh")
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.stub.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn result(&self) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.state.join("merge-ready-result.json"))
                .expect("the driver writes merge-ready-result.json"),
        )
        .unwrap()
    }

    fn head(&self) -> String {
        crate::support::quick_change_repo::git(&self.repo, &["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }
}

fn assert_never_stuck_on_shape(out: &Output) {
    let all = show(out);
    for refused in [
        "invalid agent round evidence",
        "CandidateRecord",
        "unknown field",
    ] {
        assert!(
            !all.contains(refused),
            "the write-up is never parsed ({refused}):\n{all}"
        );
    }
}

#[test]
fn prose_shape_does_not_stop_drive() {
    for (name, prose) in FIXTURES {
        let Some(d) = Drive::new() else { return };
        d.plan(&["ready|0"]);
        d.writeup(1, prose);
        let head = d.head();
        let out = d.run();
        assert_never_stuck_on_shape(&out);
        assert_eq!(out.status.code(), Some(0), "{name}: {}", show(&out));
        assert_eq!(d.result()["loop_result"], json!("DONE"), "{name}");
        assert_eq!(d.lines("round-calls"), ["round-1"], "{name}");
        assert_eq!(
            std::fs::read_to_string(d.state.join("merge-ready-latest.writeup")).unwrap(),
            prose,
            "{name}: the latest write-up is kept verbatim for finalisation and hand-off"
        );
        assert_eq!(
            std::fs::read_to_string(d.state.join("merge-ready-latest.head"))
                .unwrap()
                .trim(),
            head,
            "{name}: finalisation's expected head is the assessed head"
        );
        assert!(
            !d.lines("helper-calls")
                .iter()
                .any(|c| c.contains("complete-loop")),
            "no loop receipt is written: {:?}",
            d.lines("helper-calls")
        );
    }
}

#[test]
fn not_ready_runs_another_round_with_prior_writeup() {
    for (name, prose) in FIXTURES {
        let Some(d) = Drive::new() else { return };
        d.plan(&["not-ready|1", "ready|0"]);
        d.writeup(1, prose);
        d.writeup(2, "Round two: all 13 criteria met. Outcome: ready.\n");
        let out = d.run();
        assert_never_stuck_on_shape(&out);
        assert_eq!(out.status.code(), Some(0), "{name}: {}", show(&out));
        assert_eq!(d.lines("round-calls"), ["round-1", "round-2"], "{name}");
        assert_eq!(d.result()["loop_result"], json!("DONE"), "{name}");
        let prior = std::fs::read_to_string(d.stub.join("ctx-2-prior_writeup"))
            .unwrap_or_else(|_| panic!("{name}: round two receives prior_writeup from a file"));
        assert!(prior.contains(prose.trim_end()), "{name}: {prior:?}");
        let first = std::fs::read_to_string(d.stub.join("ctx-1-prior_writeup")).unwrap_or_default();
        assert!(
            first.trim().is_empty(),
            "round one has no prior write-up: {first:?}"
        );
        // The write-up never travels on argv.
        let marker = prose.lines().next().unwrap();
        for n in [1, 2] {
            let argv = std::fs::read(d.stub.join(format!("round-argv-{n}"))).unwrap();
            assert!(
                !String::from_utf8_lossy(&argv).contains(marker),
                "{name}: the write-up is on round {n}'s argv"
            );
        }
    }
}

#[test]
fn not_ready_rounds_at_an_unchanged_head_are_not_no_progress() {
    // Round two waits for pending CI: not ready, nothing to commit, same head.
    let Some(d) = Drive::new() else { return };
    d.plan(&["not-ready|0", "not-ready|0", "ready|0"]);
    d.writeup(1, "CI is still running. Outcome: not-ready.\n");
    d.writeup(2, "CI is still running. Outcome: not-ready.\n");
    d.writeup(3, "CI is green. Outcome: ready.\n");
    let out = d.run();
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_ne!(d.result()["loop_result"], json!("NO_PROGRESS"));
    assert_eq!(d.result()["loop_result"], json!("DONE"));
    assert_eq!(d.lines("round-calls"), ["round-1", "round-2", "round-3"]);
}

#[test]
fn unclear_word_is_another_round() {
    let Some(d) = Drive::new() else { return };
    d.plan(&["probably fine|1", "ready|0"]);
    d.writeup(1, "I think it is mostly there.\n");
    d.writeup(2, "Outcome: ready.\n");
    let out = d.run();
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(d.lines("round-calls"), ["round-1", "round-2"]);
    assert_eq!(d.result()["loop_result"], json!("DONE"));
}

#[test]
fn handoff_word_stops_the_loop_before_the_evaluator() {
    let Some(d) = Drive::new() else { return };
    d.plan(&["hand-off|0"]);
    d.writeup(
        1,
        "Blocked: the required reviewer has left the project. Outcome: hand-off.\n",
    );
    let out = d.run();
    assert_eq!(out.status.code(), Some(1), "{}", show(&out));
    assert_eq!(d.result()["loop_result"], json!("HANDOFF"));
    assert!(
        d.lines("evaluator-calls").is_empty(),
        "a hand-off is recorded before the evaluator runs"
    );
    assert_eq!(d.lines("round-calls"), ["round-1"]);
    assert_eq!(
        std::fs::read_to_string(d.state.join("merge-ready-latest.writeup")).unwrap(),
        "Blocked: the required reviewer has left the project. Outcome: hand-off.\n",
        "the hand-off reads the blocker from the latest write-up"
    );
}
