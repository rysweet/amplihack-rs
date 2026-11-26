//! Shared fixture for the local-model launch tests.
//!
//! A temporary `HOME` holds a user Codex home shaped like the one in the fixture
//! (a profile, a `hooks.json` with the plugin wrapper twice and two shell
//! guards, `auth.json`, rules and sessions), a fake `codex` and a fake
//! `amplihack-hooks`. `amplihack` is started through `isolated` (#1606), so
//! nothing from the test process reaches it: in particular no
//! `AMPLIHACK_CODEX_LOCAL_MODEL` or `AMPLIHACK_HOOK_TEXT` a local session
//! exported to `cargo test`. Each test adds only what it sets.
//!
//! The fake Codex answers the launcher's probes, then records its argv in
//! `$HOME/argv.txt` and four variables in `$HOME/env.txt` (`__UNSET__` when
//! absent). Every call, probes included, is appended to `$HOME/calls.log`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub(crate) const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
/// The value of the `-c` a local launch puts before the caller's arguments.
pub(crate) const HOOKS_ON: &str = "features.hooks=true";
pub(crate) const NAME: &str = "inkling";
pub(crate) const LOCAL_MODEL_ENV: &str = "AMPLIHACK_CODEX_LOCAL_MODEL";
pub(crate) const HOOK_TEXT_ENV: &str = "AMPLIHACK_HOOK_TEXT";
pub(crate) const HOOKS_OVERRIDE_ENV: &str = "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH";
pub(crate) const RULES_FILE: &str = "rules/amplihack-dangerous-commands.rules";
/// The four files amplihack owns in a generated home.
pub(crate) const OWNED: [&str; 4] = ["config.toml", "hooks.json", "instructions.md", RULES_FILE];
const UNSET: &str = "__UNSET__";

const FAKE_CODEX: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/calls.log"
case "$1" in
  --version)
    if [ -f "$HOME/codex-version" ]; then cat "$HOME/codex-version"; else echo 'codex-cli 0.166.0'; fi
    exit 0;;
  plugin)
    case "$2 $3" in
      list*)
        if [ -f "$HOME/plugin-installed" ]; then
          printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"source":{"path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$HOME"
        else
          echo '{"installed":[]}'
        fi;;
      'marketplace add')
        printf '\n[marketplaces.amplihack-local]\nsource_type = "local"\nsource = "%s/.amplihack/codex/market"\n' "$HOME" >> "$CODEX_HOME/config.toml"
        echo '{"ok":true}';;
      add*)
        printf '\n[plugins."amplihack@amplihack-local"]\nenabled = true\n' >> "$CODEX_HOME/config.toml"
        echo yes > "$HOME/plugin-installed"
        echo '{"ok":true}';;
      remove*) rm -f "$HOME/plugin-installed"; echo '{"ok":true}';;
      *) echo '{"ok":true}';;
    esac
    exit 0;;
esac
: > "$HOME/argv.txt"
for arg in "$@"; do
  printf '%s\n' "$arg" >> "$HOME/argv.txt"
done
: > "$HOME/env.txt"
for name in CODEX_HOME AMPLIHACK_HOOK_TEXT AMPLIHACK_CODEX_LOCAL_MODEL AMPLIHACK_HOOK_PROTOCOL; do
  eval "value=\${$name-__UNSET__}"
  printf '%s=%s\n' "$name" "$value" >> "$HOME/env.txt"
done
cat > /dev/null
exit 0
"#;

/// The profile on the example setup, plus keys amplihack must override or drop.
pub(crate) const PROFILE: &str = r#"model = "inkling-2-mini"
model_provider = "llamacpp-local"
model_context_window = 65536
web_search = "live"
project_doc_max_bytes = 32768
model_instructions_file = "/elsewhere/instructions.md"
profile = "hosted"

[model_providers.llamacpp-local]
name = "llama.cpp (local)"
base_url = "http://127.0.0.1:8080/v1"
wire_api = "responses"
stream_max_retries = 2

[features]
multi_agent = true
goals = true
view_image = true
plugins = true
apps = true
hooks = false
some_future_feature = true

[skills.bundled]
enabled = true
extra = "kept"

[profiles.hosted]
model = "gpt-5"
web_search = "live"

[marketplaces.amplihack-local]
source_type = "local"
source = "/somewhere/market"

[plugins."amplihack@amplihack-local"]
enabled = true

[projects."/work/checkout"]
trust_level = "trusted"

[[hooks.PreToolUse]]
command = "echo profile-hook"
"#;

/// The user's own `config.toml`. Its `approval_policy` is used in a generated
/// home whose profile sets none; nothing else in it is used.
pub(crate) const USER_CONFIG: &str = r#"model = "gpt-5"
approval_policy = "on-request"
web_search = "live"

[features]
multi_agent = true
"#;

/// The PreToolUse groups of the fixture's `hooks.json` that a generated home
/// keeps, in order: the plugin wrapper with its `env XPIA_ENABLED=false`
/// prefix (a handler that sets a variable is the user's) and the two shell
/// guards.
pub(crate) fn kept_user_groups(home: &Path) -> Vec<serde_json::Value> {
    let adapter = home.join(".codex/hooks/my-check.sh");
    let wrapper = home.join(".amplihack/codex/market/plugin/bin/hook");
    vec![
        serde_json::json!({"hooks": [{"type": "command",
            "command": format!("env XPIA_ENABLED=false '{}' pre-tool-use", wrapper.display())}]}),
        serde_json::json!({"matcher": "Bash", "hooks": [{"type": "command", "timeout": 10,
            "command": format!("/bin/sh {} tmux-check", adapter.display())}]}),
        serde_json::json!({"hooks": [{"type": "command",
            "command": format!("/bin/sh {} shell-check", adapter.display())}]}),
    ]
}

/// the fixture's `~/.codex/hooks.json`: the plugin wrapper (with and without an
/// `env XPIA_ENABLED=false` prefix) and the two guards under PreToolUse, and
/// wrapper entries for other events. Only the bare wrapper is dropped.
pub(crate) fn user_hooks_json(home: &Path) -> String {
    let wrapper = home.join(".amplihack/codex/market/plugin/bin/hook");
    let quoted = format!("'{}'", wrapper.display());
    let kept = kept_user_groups(home);
    serde_json::to_string_pretty(&serde_json::json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": format!("{quoted} session-start")}]}],
        "UserPromptSubmit": [{"hooks": [{"type": "command", "command": format!("{quoted} user-prompt")}]}],
        "PreToolUse": [
            kept[0],
            {"hooks": [{"type": "command", "command": format!("{quoted} pre-tool-use")}]},
            kept[1],
            kept[2],
        ],
        "Stop": [{"hooks": [{"type": "command", "command": format!("{quoted} stop")}]}],
    }}))
    .unwrap()
}

pub(crate) struct Fixture {
    dir: tempfile::TempDir,
    home: PathBuf,
    tools: PathBuf,
}

impl Fixture {
    /// A fixture whose `HOME` is the temporary directory itself.
    pub(crate) fn new() -> Self {
        Self::build(None)
    }

    /// A fixture whose `HOME` is `<tempdir>/<name>`, for awkward names.
    pub(crate) fn with_home_named(name: &str) -> Self {
        Self::build(Some(name))
    }

    fn build(home_name: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = match home_name {
            Some(name) => dir.path().join(name),
            None => dir.path().to_path_buf(),
        };
        fs::create_dir_all(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir(&tools).unwrap();
        // The hooks binary lives here, and a directory others can write is
        // rejected for it; the umask of the machine decides nothing.
        fs::set_permissions(&tools, fs::Permissions::from_mode(0o755)).unwrap();
        for tool in [
            "sh", "cat", "mkdir", "rm", "mv", "cp", "chmod", "git", "uname",
        ] {
            let found = Command::new("/bin/sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            if found.status.success() {
                std::os::unix::fs::symlink(
                    String::from_utf8(found.stdout).unwrap().trim(),
                    tools.join(tool),
                )
                .unwrap();
            }
        }
        let stub = crate::support::exec_stub::write_executable;
        stub(&tools.join("codex"), FAKE_CODEX, 0o700);
        stub(
            &tools.join("codex-code-mode-host"),
            "#!/bin/sh\nexit 0\n",
            0o700,
        );
        stub(&tools.join("node"), "#!/bin/sh\necho v22.14.0\n", 0o700);
        stub(
            &tools.join("amplihack-hooks"),
            "#!/bin/sh\necho '{}'\n",
            0o700,
        );

        // `~/.amplihack` exists and is private, as on any host that has run
        // amplihack; the umask of the machine running the tests decides
        // nothing.
        let amplihack = home.join(".amplihack");
        fs::create_dir(&amplihack).unwrap();
        fs::set_permissions(&amplihack, fs::Permissions::from_mode(0o700)).unwrap();

        let codex = home.join(".codex");
        for sub in ["", "hooks", "rules", "sessions/2026/11/24"] {
            fs::create_dir_all(codex.join(sub)).unwrap();
        }
        fs::write(codex.join("config.toml"), USER_CONFIG).unwrap();
        fs::write(codex.join("hooks.json"), user_hooks_json(&home)).unwrap();
        fs::write(codex.join(format!("{NAME}.config.toml")), PROFILE).unwrap();
        fs::write(
            codex.join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-user-own"}"#,
        )
        .unwrap();
        fs::write(
            codex.join("rules/default.rules"),
            "prefix_rule(pattern=[\"ls\"])\n",
        )
        .unwrap();
        fs::write(codex.join("hooks/my-check.sh"), "# guard\n").unwrap();
        fs::write(
            codex.join("sessions/2026/11/24/rollout.jsonl"),
            "{\"type\":\"session\"}\n",
        )
        .unwrap();
        Self { dir, home, tools }
    }

    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    pub(crate) fn user_codex(&self) -> PathBuf {
        self.home.join(".codex")
    }

    pub(crate) fn codex_local(&self) -> PathBuf {
        self.home.join(".amplihack/codex-local")
    }

    pub(crate) fn generated(&self, name: &str) -> PathBuf {
        self.codex_local().join(name)
    }

    pub(crate) fn hooks_bin(&self) -> PathBuf {
        self.tools.join("amplihack-hooks")
    }

    pub(crate) fn tools(&self) -> &Path {
        &self.tools
    }

    pub(crate) fn write_profile(&self, name: &str, text: &str) {
        fs::write(self.user_codex().join(format!("{name}.config.toml")), text).unwrap();
    }

    /// `amplihack <args>` with the fixture's environment.
    pub(crate) fn command(&self, args: &[&str]) -> Command {
        let mut command =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        command
            .args(args)
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.user_codex())
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("PATH", &self.tools)
            .env("CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .env(HOOKS_OVERRIDE_ENV, self.hooks_bin())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            // a test sets these itself or they are absent.
            .env_remove(LOCAL_MODEL_ENV)
            .env_remove(HOOK_TEXT_ENV)
            .env_remove("AMPLIHACK_HOOK_PROTOCOL")
            .env_remove("AMPLIHACK_USE_DOCKER")
            .env_remove("AMPLIHACK_CODEX_BINARY_PATH")
            .env_remove("AMPLIHACK_DEFAULT_MODEL")
            .env_remove("AMPLIHACK_SESSION_ID")
            .env_remove("AMPLIHACK_SESSION_DEPTH")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    /// [`Fixture::command`] with start-up self-heal turned on, so that the
    /// asset repair that runs before dispatch is exercised. The
    /// framework comes from this checkout, as in `codex_frontdoor_contract`.
    pub(crate) fn command_with_self_heal(&self, args: &[&str]) -> Command {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        crate::support::exec_stub::write_executable(
            &self.tools.join("recipe-runner-rs"),
            "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]}'\n",
            0o700,
        );
        let mut command = self.command(args);
        command
            .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
            .env("AMPLIHACK_SOURCE_ROOT", &source)
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"));
        command
    }

    /// Stage framework assets through the installer with a fixture runner,
    /// then return a launch with start-up self-heal disabled. Interactive
    /// bootstrap still runs, without needing to install a runner or compiler.
    pub(crate) fn command_with_staged_bundle(&self, args: &[&str]) -> Command {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // Provision the bundle before Codex is available, so staging does
        // not install the optional plugin into the user-home fixture.
        let codex = self.tools.join("codex");
        let stored_codex = self.tools.join("codex-stored");
        fs::rename(&codex, &stored_codex).unwrap();
        let out = self
            .command_with_self_heal(&["install", "--local", source.to_str().unwrap()])
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env_remove("CODEX_BINARY_PATH")
            .output()
            .expect("stage the fixture bundle");
        fs::rename(&stored_codex, &codex).unwrap();
        assert!(
            out.status.success(),
            "bundle staging failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let mut command = self.command(args);
        command
            .env("AMPLIHACK_SOURCE_ROOT", &source)
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"));
        command
    }

    /// Runs `command` and reads what the fake Codex recorded, if it ran.
    pub(crate) fn launch(&self, command: &mut Command) -> Launched {
        for record in ["argv.txt", "env.txt"] {
            let _ = fs::remove_file(self.home.join(record));
        }
        let out = command.output().expect("run amplihack codex");
        self.collect(out)
    }

    fn collect(&self, out: Output) -> Launched {
        let argv = fs::read_to_string(self.home.join("argv.txt"))
            .ok()
            .map(|text| text.lines().map(str::to_owned).collect());
        let env = fs::read_to_string(self.home.join("env.txt"))
            .ok()
            .map(|text| {
                text.lines()
                    .filter_map(|line| line.split_once('='))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect()
            });
        let calls = fs::read_to_string(self.home.join("calls.log")).unwrap_or_default();
        Launched {
            out,
            argv,
            env,
            calls,
        }
    }

    /// [`Fixture::launch`] with stdin on a pseudo-terminal and
    /// `AMPLIHACK_NONINTERACTIVE` and `CI` removed, as a person at a terminal
    /// starts it, so the launch bootstrap runs. The terminal already holds an
    /// end-of-file, so a read of stdin returns at once; stdout and stderr are
    /// captured as usual.
    pub(crate) fn launch_on_terminal(&self, command: &mut Command) -> Launched {
        use std::io::Write;
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: both out-pointers are valid; the name, termios and winsize
        // pointers may be null. A typed null `*mut` compiles on Linux and
        // macOS alike (#2210).
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut::<libc::termios>(),
                std::ptr::null_mut::<libc::winsize>(),
            )
        };
        assert_eq!(opened, 0, "openpty");
        // SAFETY: openpty returned two fresh descriptors this code now owns.
        let mut input = unsafe { fs::File::from_raw_fd(master) };
        let terminal = unsafe { fs::File::from_raw_fd(slave) };
        input.write_all(b"\x04").unwrap();
        command
            .env_remove("AMPLIHACK_NONINTERACTIVE")
            .env_remove("CI")
            .stdin(Stdio::from(terminal));
        let launched = self.launch(command);
        drop(input);
        launched
    }

    /// `--local-model <name>` and, optionally, a Codex argv tail.
    pub(crate) fn local(&self, name: &str, tail: &[&str]) -> Launched {
        let mut args = vec!["codex", "--local-model", name];
        if !tail.is_empty() {
            args.push("--");
            args.extend_from_slice(tail);
        }
        self.launch(&mut self.command(&args))
    }
}

pub(crate) struct Launched {
    pub(crate) out: Output,
    pub(crate) argv: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    pub(crate) calls: String,
}

impl Launched {
    pub(crate) fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }

    pub(crate) fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }

    pub(crate) fn started(&self) -> bool {
        self.argv.is_some()
    }

    /// The child's value of `name`, `None` when it was absent.
    pub(crate) fn env(&self, name: &str) -> Option<&str> {
        let env = self.env.as_ref().expect("the fake codex ran");
        let value = env
            .get(name)
            .unwrap_or_else(|| panic!("{name} is not one of the recorded variables"));
        (value != UNSET).then_some(value.as_str())
    }

    pub(crate) fn argv(&self) -> &[String] {
        self.argv.as_deref().expect("the fake codex ran")
    }

    pub(crate) fn count(&self, token: &str) -> usize {
        self.argv().iter().filter(|a| *a == token).count()
    }

    #[track_caller]
    pub(crate) fn assert_started(&self) {
        assert!(
            self.out.status.success() && self.started(),
            "the launch did not start Codex\nstatus: {}\nstdout: {}\nstderr: {}\ncalls: {}",
            self.out.status,
            self.stdout(),
            self.stderr(),
            self.calls
        );
    }

    /// A refused launch: non-zero, Codex never started, nothing under
    /// `codex-local`.
    #[track_caller]
    pub(crate) fn assert_refused(&self, fixture: &Fixture, what: &str) {
        assert!(
            !self.out.status.success(),
            "{what}: the launch must fail\nstderr: {}",
            self.stderr()
        );
        assert!(
            !self.started(),
            "{what}: Codex must not start: {:?}",
            self.argv
        );
        assert!(
            !fixture.codex_local().exists(),
            "{what}: nothing may be created under codex-local"
        );
    }

    /// Refused by amplihack at run time, not by argument parsing (clap exits
    /// 2 for an unknown or malformed argument).
    #[track_caller]
    pub(crate) fn assert_refused_at_run_time(&self, fixture: &Fixture, what: &str) {
        self.assert_refused(fixture, what);
        assert_ne!(
            self.out.status.code(),
            Some(2),
            "{what}: a run-time refusal, not a usage error\nstderr: {}",
            self.stderr()
        );
    }
}

/// One entry of a recursive snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Dir { mode: u32 },
    File { mode: u32, bytes: Vec<u8> },
    Link { target: PathBuf },
    Other { mode: u32 },
}

/// Every entry under `root`, by path relative to it.
pub(crate) fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            let mode = meta.mode() & 0o7777;
            let kind = meta.file_type();
            if kind.is_symlink() {
                out.insert(
                    rel,
                    Entry::Link {
                        target: fs::read_link(&path).unwrap(),
                    },
                );
            } else if kind.is_dir() {
                out.insert(rel, Entry::Dir { mode });
                walk(&path, root, out);
            } else if kind.is_file() {
                out.insert(
                    rel,
                    Entry::File {
                        mode,
                        bytes: fs::read(&path).unwrap(),
                    },
                );
            } else {
                out.insert(rel, Entry::Other { mode });
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

pub(crate) fn inode(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

pub(crate) fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

pub(crate) fn inodes(home: &Path) -> BTreeMap<&'static str, u64> {
    OWNED.iter().map(|f| (*f, inode(&home.join(f)))).collect()
}

pub(crate) fn read_toml(path: &Path) -> toml::Value {
    toml::from_str(&fs::read_to_string(path).unwrap())
        .unwrap_or_else(|e| panic!("{} parses as TOML: {e}", path.display()))
}

pub(crate) fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap())
        .unwrap_or_else(|e| panic!("{} parses as JSON: {e}", path.display()))
}

/// The handlers of one hooks.json group.
pub(crate) fn commands(group: &serde_json::Value) -> Vec<String> {
    group["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["command"].as_str().map(str::to_owned))
        .collect()
}

/// A random-looking secret no message may ever contain.
pub(crate) fn sentinel() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("sk-SENTINEL-{}-{nanos:x}", std::process::id())
}
