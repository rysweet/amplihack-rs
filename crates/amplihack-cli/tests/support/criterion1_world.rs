//! Fixture for #2569: the criterion-1 QA judgement.
//!
//! One isolated world per test. `HOME`, `TMPDIR`, `XDG_STATE_HOME`,
//! `AMPLIHACK_HOME` and `PATH` are private to the test, so the host's own
//! `gadugi-test`, `~/.claude` and `~/.amplihack` are never reached. No fake
//! sleeps or applies a timeout.
//!
//! The measurement (`autodrive_qa_evidence.sh`) is the real one. The fake
//! judgement agent and the recipe-step runner went with the merge-ready
//! round (#3129).

use crate::isolated_amplihack::isolated;
use crate::support::amplihack_bin::amplihack_bin;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

pub fn bundle_tools() -> PathBuf {
    repo_root().join("amplifier-bundle/tools")
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

pub fn field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("no string field {key} in {v}"))
        .to_string()
}

pub struct World {
    _tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub tmpdir: PathBuf,
    pub xdg: PathBuf,
    pub amplihack_home: PathBuf,
    pub bin: PathBuf,
    pub repo: PathBuf,
    pub state: PathBuf,
    pub gadugi_log: PathBuf,
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
            tool_state: base.join("tool-state"),
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

    pub fn feature_pr(&self) -> String {
        self.write(
            "src/app.js",
            "console.log(\"shop\");\nif (process.argv[2] === \"checkout\") { console.log(\"Order placed: \" + total); }\n",
        );
        self.commit("feat: checkout command")
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

    /// Every recorded `gadugi-test` call, in order, one log line each.
    pub fn gadugi_calls(&self) -> Vec<String> {
        fs::read_to_string(&self.gadugi_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}
