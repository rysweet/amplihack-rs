//! Issue #1802: fixtures shared by the `issue_1802_auto_drive_light*` tests.
//!
//! - [`LightRepo`]: a temporary repository with a bare `origin`, a pinned
//!   base commit and the files the eligibility rules look at.
//! - [`fake_bin`]: a directory holding a fake `gh` and a fake `amplihack`,
//!   both of which log their argv. The fake `gh` answers from canned JSON and
//!   applies `--jq` with the real `jq`, so the tools may ask in either form.
//! - [`run_bash`]: runs a tool or an extracted recipe step in `bash` started
//!   by [`crate::isolated_amplihack::isolated`], with a temporary `HOME`,
//!   no `AMPLIHACK_CLAUDE_PROFILE_ROOT`, and the fakes first on `PATH`.
//!
//! No agent, real `gh`, network, tmux server or real `~/.amplihack` is used.

#![allow(dead_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

pub use crate::support::quick_change_repo::git;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

pub fn tools_dir() -> PathBuf {
    repo_root().join("amplifier-bundle/tools")
}

pub fn eligibility_tool() -> PathBuf {
    tools_dir().join("autodrive_light_eligibility.sh")
}

pub fn round_tool() -> PathBuf {
    tools_dir().join("autodrive_light_round.sh")
}

pub fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

pub fn recipe(name: &str) -> serde_yaml::Value {
    serde_yaml::from_str(&read(&format!("amplifier-bundle/recipes/{name}.yaml")))
        .unwrap_or_else(|e| panic!("{name}.yaml must parse: {e}"))
}

pub fn step_command(recipe_name: &str, step_id: &str) -> String {
    recipe(recipe_name)["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .and_then(|s| s["command"].as_str())
        .unwrap_or_else(|| panic!("{recipe_name}: step {step_id} has no command"))
        .to_string()
}

pub fn jq_available() -> bool {
    Command::new("jq")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub fn show(o: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        stdout(o),
        stderr(o)
    )
}

/// Makes `dir` private (0700), as the autodrive state directory is.
pub fn private_dir(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Writes `body` to `path` with mode 0600.
pub fn private_file(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// A temporary repository: `origin.git` (bare) and a clone `wt` on `main`,
/// with a base commit holding the files the eligibility rules inspect.
pub struct LightRepo {
    pub tmp: TempDir,
    pub wt: PathBuf,
    pub home: PathBuf,
    pub base: String,
}

pub const BASE_RECIPE: &str = "name: demo\ncontext:\n  alpha: \"1\"\n  beta: \"\"\nsteps:\n  - id: one\n    type: bash\n    command: echo one\n";

impl LightRepo {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let wt = tmp.path().join("wt");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        git(
            tmp.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                wt.to_str().unwrap(),
            ],
        );
        git(&wt, &["checkout", "-q", "-b", "main"]);
        let mut r = LightRepo {
            tmp,
            wt,
            home,
            base: String::new(),
        };
        r.write("README.md", "# demo\n");
        r.write("Cargo.toml", "[workspace]\nmembers = [\"crates/foo\"]\n");
        r.write("Cargo.lock", &"# lock\n".repeat(10));
        r.write(
            "crates/foo/Cargo.toml",
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
        );
        r.write(
            "crates/foo/src/lib.rs",
            "fn private() {}\n\npub fn kept() {}\n",
        );
        r.write("crates/foo/src/existing.rs", "fn helper() {}\n");
        r.write("amplifier-bundle/recipes/demo.yaml", BASE_RECIPE);
        r.write("amplifier-bundle/tools/existing.sh", "#!/bin/sh\necho hi\n");
        r.write("docs/guide.md", "# guide\n");
        r.commit_all("base");
        git(&r.wt, &["push", "-q", "-u", "origin", "main"]);
        r.base = r.head();
        r
    }

    pub fn write(&self, rel: &str, body: &str) {
        let p = self.wt.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    pub fn write_bytes(&self, rel: &str, body: &[u8]) {
        let p = self.wt.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    pub fn remove(&self, rel: &str) {
        std::fs::remove_file(self.wt.join(rel)).unwrap();
    }

    pub fn commit_all(&self, message: &str) {
        git(&self.wt, &["add", "-A"]);
        git(&self.wt, &["commit", "-q", "--allow-empty", "-m", message]);
    }

    pub fn head(&self) -> String {
        git(&self.wt, &["rev-parse", "HEAD"])
    }

    /// Commits the working tree and returns the new head.
    pub fn commit(&self, message: &str) -> String {
        self.commit_all(message);
        self.head()
    }

    /// Runs the eligibility tool on `base..HEAD` with `extra` options.
    pub fn measure(&self, extra: &[&str]) -> Output {
        let head = self.head();
        let mut args: Vec<String> = vec![
            "--repo".into(),
            self.wt.to_string_lossy().into_owned(),
            "--base".into(),
            self.base.clone(),
            "--head".into(),
            head,
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        self.eligibility(&args)
    }

    pub fn eligibility(&self, args: &[String]) -> Output {
        let mut c = crate::isolated_amplihack::isolated("bash");
        c.arg(eligibility_tool())
            .args(args)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null());
        c.output().expect("run autodrive_light_eligibility.sh")
    }
}

/// Asserts exit 0 and the decision line, and returns the JSON on line 2.
pub fn decision(o: &Output, line1: &str) -> serde_json::Value {
    assert_eq!(o.status.code(), Some(0), "{}", show(o));
    let out = stdout(o);
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some(line1), "{}", show(o));
    let j: serde_json::Value =
        serde_json::from_str(lines.next().expect("JSON on line 2")).expect("line 2 is JSON");
    assert!(lines.next().is_none(), "exactly two lines: {}", show(o));
    j
}

/// Asserts a fault: exit 2, nothing on stdout, and the fault line on stderr.
pub fn fault(o: &Output, token: &str) {
    assert_eq!(o.status.code(), Some(2), "{}", show(o));
    assert!(
        stdout(o).trim().is_empty(),
        "no stdout on a fault: {}",
        show(o)
    );
    let want = format!("AUTODRIVE_LIGHT_ELIGIBILITY=fault reason={token}");
    assert!(
        stderr(o).lines().any(|l| l == want),
        "stderr must carry `{want}`: {}",
        show(o)
    );
}

/// A fake `gh`. Every call appends its argv to `$FAKE_LOG/gh.log` as one
/// line of arguments joined by U+001F, and copies any `--body-file` to
/// `$FAKE_LOG/body-<n>.md`. Answers come from canned JSON, with `--jq`
/// applied by the real `jq`:
///
/// - `api user`                  `{"login":"$FAKE_GH_LOGIN"}` (default `me`)
/// - `repo view`                 `{"nameWithOwner":"o/r","url":"https://github.com/o/r"}`
/// - `pr view`                   PR 7 of `o/r`, head `$FAKE_GH_HEAD`
/// - `label list`                `$FAKE_GH_LABELS` (default `[]`)
/// - `search issues`/`issue list` `$FAKE_GH_SEARCH` (default `[]`)
/// - `api .../search/issues...`  `{"items": $FAKE_GH_SEARCH_API}` (default `[]`)
/// - `issue view <n>`            `$FAKE_GH_ISSUE_VIEW` (default open, by `me`)
/// - `issue create`              `https://github.com/o/r/issues/$FAKE_GH_NEW_ISSUE`
/// - `issue comment`, `pr comment` nothing
///
/// When `$FAKE_GH_FAIL` is set and the joined argv contains it, the call
/// prints `HTTP 404: Not Found` on stderr and exits `$FAKE_GH_EXIT_CODE`
/// (default 1; a 404 is not retried).
pub const FAKE_GH: &str = r#"#!/usr/bin/env bash
set -u
log="${FAKE_LOG:?}"
n=$(( $(wc -l < "$log/gh.log" 2>/dev/null || echo 0) + 1 ))
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "$log/gh.log"
args=("$@")
jqf=""
for ((i=0; i<${#args[@]}; i++)); do
  case "${args[$i]}" in
    --jq|-q) jqf="${args[$((i+1))]:-}" ;;
    --jq=*) jqf="${args[$i]#--jq=}" ;;
    --body-file|-F) [ "${args[$i]}" = "--body-file" ] && cp -- "${args[$((i+1))]}" "$log/body-$n.md" 2>/dev/null ;;
  esac
done
joined="$*"
if [ -n "${FAKE_GH_FAIL:-}" ] && [[ "$joined" == *"$FAKE_GH_FAIL"* ]]; then
  echo "HTTP 404: Not Found" >&2
  exit "${FAKE_GH_EXIT_CODE:-1}"
fi
emit() { if [ -n "$jqf" ]; then printf '%s' "$1" | jq -r "$jqf"; else printf '%s\n' "$1"; fi; }
login="${FAKE_GH_LOGIN:-me}"
case "$1 ${2:-}" in
  "api user") emit "{\"login\":\"$login\"}" ;;
  "repo view") emit '{"nameWithOwner":"o/r","url":"https://github.com/o/r","owner":{"login":"o"},"name":"r"}' ;;
  "pr view") emit "{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"isCrossRepository\":false,\"headRefOid\":\"${FAKE_GH_HEAD:-}\",\"headRefName\":\"quick/demo\",\"baseRefName\":\"main\",\"state\":\"OPEN\",\"headRepository\":{\"name\":\"r\"},\"headRepositoryOwner\":{\"login\":\"o\"}}" ;;
  "label list") emit "${FAKE_GH_LABELS:-[]}" ;;
  "search issues"|"issue list") emit "${FAKE_GH_SEARCH:-[]}" ;;
  "issue view") emit "${FAKE_GH_ISSUE_VIEW:-{\"number\":${3:-0},\"state\":\"OPEN\",\"author\":{\"login\":\"$login\"},\"body\":\"\"}}" ;;
  "issue create") printf 'https://github.com/o/r/issues/%s\n' "${FAKE_GH_NEW_ISSUE:-1851}" ;;
  "issue comment"|"pr comment") : ;;
  *)
    if [ "$1" = api ] && [[ "$joined" == *search/issues* ]]; then
      emit "{\"total_count\":0,\"items\":${FAKE_GH_SEARCH_API:-[]}}"
    elif [ "$1" = api ] && [[ "$joined" == *repos/o/r/issues/* ]]; then
      emit "${FAKE_GH_ISSUE_API:-{\"state\":\"open\",\"user\":{\"login\":\"$login\"},\"body\":\"\"}}"
    elif [ "$1" = api ]; then
      emit '{}'
    fi
    ;;
esac
exit 0
"#;

/// A fake `amplihack`. Logs argv to `$FAKE_LOG/amplihack.log`.
/// `orch helper extract-json` prints stdin when it is JSON holding the
/// `--require-field`, else the first such line, else nothing; `extract-field
/// --field F --default D` prints `.F` (strings raw, anything else as compact
/// JSON) or D.
pub const FAKE_AMPLIHACK: &str = r#"#!/usr/bin/env bash
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "${FAKE_LOG:?}/amplihack.log"
if [ "${1:-} ${2:-}" != "orch helper" ]; then exit 0; fi
case "${3:-}" in
  extract-json)
    in="$(cat)"; field="${5:-}"
    ok() { printf '%s' "$1" | jq -e --arg f "$field" 'type == "object" and has($f)' >/dev/null 2>&1; }
    if ok "$in"; then printf '%s\n' "$in"; exit 0; fi
    while IFS= read -r l; do
      if ok "$l"; then printf '%s\n' "$l"; exit 0; fi
    done <<< "$in" ;;
  extract-field)
    in="$(cat)"
    out="$(printf '%s' "$in" | jq -r --arg f "${5:-}" 'if type == "object" and has($f) then (.[$f] | if type == "string" then . else tojson end) else empty end' 2>/dev/null)"
    if [ -n "$out" ]; then printf '%s\n' "$out"; else printf '%s\n' "${7:-}"; fi ;;
esac
exit 0
"#;

/// A directory holding the fake `gh` and `amplihack`, and the log dir.
pub struct Fakes {
    pub bin: PathBuf,
    pub log: PathBuf,
}

pub fn fake_bin(root: &Path) -> Fakes {
    let bin = root.join("fake-bin");
    let log = root.join("fake-log");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&log).unwrap();
    crate::support::exec_stub::write_executable(&bin.join("gh"), FAKE_GH, 0o755);
    crate::support::exec_stub::write_executable(&bin.join("amplihack"), FAKE_AMPLIHACK, 0o755);
    Fakes { bin, log }
}

impl Fakes {
    /// Every `gh` call, each as its argv.
    pub fn gh_calls(&self) -> Vec<Vec<String>> {
        read_log(&self.log.join("gh.log"))
    }

    pub fn amplihack_calls(&self) -> Vec<Vec<String>> {
        read_log(&self.log.join("amplihack.log"))
    }

    /// The `--body-file` copies, in call order.
    pub fn bodies(&self) -> Vec<String> {
        let mut v: Vec<(usize, String)> = std::fs::read_dir(&self.log)
            .unwrap()
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name().into_string().ok()?;
                let n = name
                    .strip_prefix("body-")?
                    .strip_suffix(".md")?
                    .parse()
                    .ok()?;
                Some((n, std::fs::read_to_string(e.path()).ok()?))
            })
            .collect();
        v.sort();
        v.into_iter().map(|(_, b)| b).collect()
    }

    pub fn path_with_fakes(&self) -> std::ffi::OsString {
        std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )),
        )
        .unwrap()
    }
}

fn read_log(p: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('\u{1f}').map(str::to_string).collect())
        .collect()
}

/// True when some call has `a` and `b` as its first two arguments.
pub fn called(calls: &[Vec<String>], a: &str, b: &str) -> bool {
    calls.iter().any(|c| {
        c.first().map(String::as_str) == Some(a) && c.get(1).map(String::as_str) == Some(b)
    })
}

pub fn calls_of<'a>(calls: &'a [Vec<String>], a: &str, b: &str) -> Vec<&'a Vec<String>> {
    calls
        .iter()
        .filter(|c| {
            c.first().map(String::as_str) == Some(a) && c.get(1).map(String::as_str) == Some(b)
        })
        .collect()
}

/// Runs `bash <args...>` (or `bash -c <script>` when `script` is set)
/// isolated, with `home` as `HOME`, the fakes first on `PATH`, `env` added,
/// and `stdin` piped in.
pub fn run_bash(
    home: &Path,
    fakes: &Fakes,
    script: Option<&str>,
    args: &[&std::ffi::OsStr],
    env: &[(&str, &std::ffi::OsStr)],
    stdin: &str,
) -> Output {
    use std::io::Write;
    let mut c = crate::isolated_amplihack::isolated("bash");
    if let Some(s) = script {
        c.arg("-c").arg(s).arg("bash");
    }
    c.args(args)
        .env("HOME", home)
        .env("PATH", fakes.path_with_fakes())
        .env("FAKE_LOG", &fakes.log)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().expect("spawn bash");
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).ok();
    child.wait_with_output().expect("wait for bash")
}
