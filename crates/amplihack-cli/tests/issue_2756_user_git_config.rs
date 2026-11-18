//! Issue #2756: a recipe step reads the user's own Git configuration again.
//!
//! Since #2290 `RunCheckout::apply_to` set `GIT_CONFIG_NOSYSTEM=1`,
//! `GIT_CONFIG_SYSTEM=/dev/null` and `GIT_CONFIG_GLOBAL=/dev/null` on the
//! recipe-runner command itself, so every step, hook and agent started under a
//! run lost the user's global and system Git configuration. A URL-scoped
//! `credential.https://github.com.helper` went with it, and a step's `git
//! fetch` or `git push` over https then ended with `could not read Username
//! for 'https://github.com'`.
//!
//! The two behaviours pinned here are the two halves of that change:
//!
//! 1. the user's own global configuration — a commit identity and a
//!    URL-scoped credential helper — reaches a step;
//! 2. #2096's protection is untouched: inherited `GIT_DIR` and
//!    `GIT_INDEX_FILE` still cannot send a run's Git back to the checkout
//!    they name.
//!
//! The runner is a stub shell script wired through `RECIPE_RUNNER_RS_PATH`.
//! `RunCheckout::apply_to` is applied to exactly that command, at both of its
//! call sites in `execute.rs`, so the stub's environment is the environment
//! every step would inherit. The stub runs the real Git operations rather
//! than asserting on a list of variable names.
//!
//! Nothing here reads or writes the developer's real `~/.gitconfig`,
//! `~/.claude` or `~/.amplihack`: `Fixture::base_environment` clears the
//! environment and repoints `HOME`, `TMPDIR` and every tool-state variable
//! into the temp root, and `AMPLIHACK_TEST_HOME_GUARD` makes the CLI discard
//! any `HOME` that is not under the temp dir, so a fixture that leaked would
//! read no user config rather than the developer's. Nothing contacts
//! github.com either -- the helper is a local script and the stub performs no
//! network operation.
#![cfg(unix)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// The commit identity that exists only in the temporary `HOME`'s
/// `.gitconfig`. The caller's checkout deliberately has no `--local`
/// `user.name`, because `RunCheckout::prepare` copies a local one into the
/// private clone and would mask what this test is asking about.
const SENTINEL_NAME: &str = "Issue 2756 Global Identity";
const SENTINEL_EMAIL: &str = "issue-2756@example.invalid";

/// What the fake credential helper answers with. Not a credential: two fixed
/// strings this test wrote itself, moments earlier, under its own temp root.
const HELPER_USERNAME: &str = "issue-2756-helper-user";
const HELPER_PASSWORD: &str = "issue-2756-helper-token";

/// The fake credential helper. `{marker}` is replaced with an absolute path
/// under the fixture's temp root before the file is written, so the marker
/// does not depend on a variable surviving into git's child.
const FAKE_HELPER: &str = r#"#!/bin/sh
# Issue #2756 fake credential helper. Local only; it contacts nothing.
set -eu
if [ "${1:-}" = get ]; then
  printf 'get\n' >> '{marker}'
  printf 'username=%s\n' '{username}'
  printf 'password=%s\n' '{password}'
fi
exit 0
"#;

/// The stub recipe runner. It answers the launch-time capability probe,
/// enters the runner's `-C` directory, and records what Git resolves there:
/// the commit identity, and the credential Git fills for an https github.com
/// URL. With `ISSUE_2756_STAGE` set it also stages and commits a file, so a
/// leaked `GIT_DIR` or `GIT_INDEX_FILE` would show up as a change in the
/// repository those variables name.
const STUB_RUNNER: &str = r#"#!/bin/sh
set -eu
if [ "${1:-}" = --capabilities ]; then
  printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}'
  exit 0
fi
out="$ISSUE_2756_OBSERVATIONS"
mkdir -p "$out"
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    -C) dir="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cd "$dir"
printf '%s' "$(pwd -P)" > "$out/cwd"
printf '%s' "${AMPLIHACK_RUN_CHECKOUT:-}" > "$out/run_checkout"
status=0
git rev-parse --absolute-git-dir > "$out/git_dir" 2> "$out/git_dir.err" || status=$?
printf '%s' "$status" > "$out/git_dir.status"

status=0
git config --get user.name > "$out/user_name" 2> "$out/user_name.err" || status=$?
printf '%s' "$status" > "$out/user_name.status"

status=0
printf 'protocol=https\nhost=github.com\n\n' \
  | git credential fill > "$out/credential" 2> "$out/credential.err" || status=$?
printf '%s' "$status" > "$out/credential.status"

if [ -n "${ISSUE_2756_STAGE:-}" ]; then
  printf 'private output\n' > run-output
  git add -- run-output
  git -c user.name='Issue 2756 Step' -c user.email='step@example.invalid' \
    commit -qm "the run's own commit"
fi
printf '{"recipe_name":"issue-2756","success":true,"step_results":[],"context":{}}\n'
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    /// The caller's checkout: where `amplihack recipe run` is started.
    repo: PathBuf,
    /// A second, unrelated repository, for the inherited-variable test.
    other: PathBuf,
    home: PathBuf,
    runner: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = crate::support::private_tempdir::private_tempdir();
        let root = fs::canonicalize(temp.path()).unwrap();
        let repo = root.join("caller");
        let other = root.join("other");
        let home = root.join("home");
        for dir in [&repo, &other, &home] {
            crate::support::private_tempdir::private_dir_all(dir);
        }
        let runner = root.join("recipe-runner-rs");
        let helper = root.join("fake-credential-helper");
        let marker = root.join("helper-marker");
        crate::support::exec_stub::write_executable(&runner, STUB_RUNNER, 0o755);
        crate::support::exec_stub::write_executable(
            &helper,
            FAKE_HELPER
                .replace("{marker}", marker.to_str().unwrap())
                .replace("{username}", HELPER_USERNAME)
                .replace("{password}", HELPER_PASSWORD),
            0o755,
        );

        let f = Self {
            _temp: temp,
            root,
            repo,
            other,
            home,
            runner,
            marker,
        };

        // The user's own global configuration. The empty `helper =` resets
        // the accumulated helper list first: this fixture deliberately leaves
        // `GIT_CONFIG_SYSTEM` and `GIT_CONFIG_NOSYSTEM` unset, so the host's
        // /etc/gitconfig is read, and a host-level GitHub helper would
        // otherwise answer before the fake one is reached.
        fs::write(
            f.home.join(".gitconfig"),
            format!(
                "[user]\n\tname = {SENTINEL_NAME}\n\temail = {SENTINEL_EMAIL}\n\
                 [credential \"https://github.com\"]\n\thelper =\n\thelper = !{}\n",
                helper.display()
            ),
        )
        .unwrap();

        // No `--local` user.name or user.email anywhere: the identity must
        // come from the global file, which is the case the issue describes.
        for dir in [f.repo.clone(), f.other.clone()] {
            f.git_in(&dir, &["init", "-q"]);
        }
        fs::write(f.repo.join("tracked"), b"caller\n").unwrap();
        f.git_in(&f.repo, &["add", "tracked"]);
        f.commit_in(&f.repo, "caller base");
        fs::write(f.other.join("tracked"), b"other\n").unwrap();
        f.git_in(&f.other, &["add", "tracked"]);
        f.commit_in(&f.other, "other base");

        fs::write(
            f.root.join("recipe.yaml"),
            "name: issue-2756\nsteps:\n  - id: stub\n    type: bash\n    command: \"true\"\n",
        )
        .unwrap();
        f
    }

    /// The temporary home and the test's own `PATH`, with no Claude or
    /// amplihack state outside the temp root. Both the fixture's Git and the
    /// `amplihack` command start from here.
    fn base_environment(&self, c: &mut Command) {
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("TMPDIR", &self.root)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"));
        // `env_clear()` dropped them; a child of this test never waits on a
        // credential prompt.
        crate::isolated_amplihack::no_git_prompt(c);
        crate::isolated_amplihack::reapply_proc_root(c);
        // Keeps a debug build's `--version` capability probe from giving up
        // after its default wait on a loaded host.
        c.env(crate::isolated_amplihack::PROBE_WAIT_ENV, "exit");
    }

    /// The fixture's own Git keeps the isolated configuration: it must not
    /// read the `.gitconfig` written above, or its commits would be made
    /// under the very identity the test is asking the run about.
    fn git_in(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        let mut c = amplihack_git::command();
        self.base_environment(&mut c);
        c.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        let out = c.current_dir(dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// A commit with an identity given on the command line, so no `--local`
    /// `user.name` is left behind for `RunCheckout::prepare` to copy.
    fn commit_in(&self, dir: &Path, message: &str) {
        self.git_in(
            dir,
            &[
                "-c",
                "user.name=Issue 2756 Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                message,
            ],
        );
    }

    /// The `amplihack` command under test. It sets none of
    /// `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM` or `GIT_CONFIG_NOSYSTEM`:
    /// what a step sees is then whatever the CLI itself decides.
    fn command(&self, label: &str) -> Command {
        let mut c =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        self.base_environment(&mut c);
        c.env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_WORK_ROOT", self.root.join("work"))
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("ISSUE_2756_OBSERVATIONS", self.root.join(label))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .current_dir(&self.repo)
            .args(["recipe", "run"])
            .arg(self.root.join("recipe.yaml"))
            .arg("-w")
            .arg(&self.repo);
        c
    }

    fn observed_text(&self, label: &str, name: &str) -> String {
        let bytes = fs::read(self.root.join(label).join(name))
            .unwrap_or_else(|e| panic!("observation {label}/{name}: {e}"));
        String::from_utf8(bytes)
            .unwrap_or_else(|e| panic!("observation {label}/{name} is not UTF-8: {e}"))
    }

    /// HEAD, the index bytes and the tracked file of the second repository.
    fn other_snapshot(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            self.git_in(&self.other, &["rev-parse", "HEAD"]),
            fs::read(self.other.join(".git/index")).unwrap(),
            fs::read(self.other.join("tracked")).unwrap(),
        )
    }
}

fn succeeds(out: &Output) {
    assert!(
        out.status.success(),
        "status {}\nstdout {}\nstderr {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git credential fill`'s output as a map. Used for assertions only; the
/// values never reach an assertion message.
fn credential_fields(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A description of `git credential fill`'s output that names the fields and
/// the length of each value, and prints no value. The raw output stays on
/// disk under the fixture's temp root for anyone debugging a failure.
fn redacted(fields: &BTreeMap<String, String>) -> String {
    let described: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{k}=<{} bytes>", v.len()))
        .collect();
    format!("[{}]", described.join(", "))
}

/// The user's own global Git configuration reaches a step.
///
/// The assertions are positive on purpose. `GIT_ASKPASS=true` is set on every
/// child here, so a `git credential fill` that never reaches a helper returns
/// empty values and exits zero: an exit-status assertion alone would pass
/// whether or not the helper was consulted.
#[test]
fn a_step_resolves_the_users_global_identity_and_credential_helper() {
    let f = Fixture::new();
    succeeds(&f.command("user-config").output().unwrap());

    assert_eq!(
        f.observed_text("user-config", "user_name").trim(),
        SENTINEL_NAME,
        "a step did not resolve the user's global commit identity (git exited {}, stderr {})",
        f.observed_text("user-config", "user_name.status"),
        f.observed_text("user-config", "user_name.err").trim()
    );

    let fields = credential_fields(&f.observed_text("user-config", "credential"));
    assert!(
        f.marker.exists(),
        "the user's credential helper was never consulted; \
         `git credential fill` exited {} and answered {}",
        f.observed_text("user-config", "credential.status"),
        redacted(&fields)
    );

    // Deliberately not `assert_eq!`: it prints the value it compared. The
    // failure this guards against is the fixture's `helper =` reset ceasing
    // to clear the list, which lets a host-level helper answer first -- and
    // its answer is a real credential. Compare here, report only `redacted`.
    assert!(
        fields.get("username").map(String::as_str) == Some(HELPER_USERNAME),
        "the filled user name is not the helper's: {}",
        redacted(&fields)
    );
    assert!(
        fields.get("password").map(String::as_str) == Some(HELPER_PASSWORD),
        "the filled password is not the helper's: {}",
        redacted(&fields)
    );
}

/// #2096's protection is untouched by the restoration: inherited `GIT_DIR`
/// and `GIT_INDEX_FILE` naming a second repository neither move the run out
/// of its private checkout nor change that repository.
#[test]
fn inherited_git_dir_and_index_file_still_cannot_reroute_a_run() {
    let f = Fixture::new();
    let before = f.other_snapshot();

    let out = f
        .command("routing")
        .env("GIT_DIR", f.other.join(".git"))
        .env("GIT_INDEX_FILE", f.other.join(".git/index"))
        .env("ISSUE_2756_STAGE", "1")
        .output()
        .unwrap();
    succeeds(&out);

    let cwd = PathBuf::from(f.observed_text("routing", "cwd"));
    let checkout = PathBuf::from(f.observed_text("routing", "run_checkout"));
    assert!(
        !checkout.as_os_str().is_empty(),
        "the run reported no private checkout"
    );
    assert_eq!(cwd, checkout, "the runner did not start in its checkout");
    assert_ne!(cwd, f.repo, "the run used the caller's checkout");
    assert!(
        !cwd.starts_with(&f.other),
        "the run was rerouted into the inherited repository: {}",
        cwd.display()
    );
    // The negative assertion below is satisfied by an empty path, so the
    // observation has to be shown good first: a `rev-parse` that failed for
    // an unrelated reason must not read as a pass.
    assert_eq!(
        f.observed_text("routing", "git_dir.status"),
        "0",
        "a step's `git rev-parse --absolute-git-dir` failed (stderr {})",
        f.observed_text("routing", "git_dir.err").trim()
    );
    let git_dir = PathBuf::from(f.observed_text("routing", "git_dir"));
    assert!(
        !git_dir.as_os_str().is_empty(),
        "a step's git resolved no git directory (stderr {})",
        f.observed_text("routing", "git_dir.err").trim()
    );
    assert!(
        !git_dir.starts_with(&f.other),
        "a step's git resolved the inherited GIT_DIR: {}",
        git_dir.display()
    );

    assert_eq!(
        f.other_snapshot(),
        before,
        "the inherited repository's HEAD, index or working tree changed"
    );
    assert!(
        !f.other.join("run-output").exists(),
        "the step's commit landed in the inherited repository"
    );
}
