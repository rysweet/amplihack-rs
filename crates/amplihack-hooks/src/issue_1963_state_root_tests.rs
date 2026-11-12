//! Issue #1963: hooks write their state under the amplihack state root, never
//! into the session's working directory.
//!
//! Every test drives a hook through its public `process()` entry point, so
//! the tests describe observable behaviour and do not depend on the internal
//! helper signatures. Each test runs with:
//!
//! - the process-wide `env_lock()` held for its whole duration;
//! - `HOME` set to a throwaway directory and `AMPLIHACK_HOME` removed, so the
//!   state root is `<home>/.amplihack` (or neither variable set, for the
//!   no-root cases);
//! - the working directory set to a separate throwaway project directory,
//!   whose full contents are compared before and after the hook runs.
//!
//! See `docs/reference/hook-state-root.md` for the contract under test.

use crate::post_tool_use::PostToolUseHook;
use crate::pre_compact::PreCompactHook;
use crate::protocol::Hook;
use crate::session_start::SessionStartHook;
use crate::test_support::{
    CwdGuard, EnvVarGuard, IsolatedStateHome, env_lock, isolate_state_home, scratch_base,
};
use crate::user_prompt::UserPromptSubmitHook;
use crate::workflow_classification::WorkflowClassificationReminderHook;
use amplihack_types::HookInput;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::MutexGuard;

/// `TOOL_CALL_THRESHOLD` in `post_tool_use::workflow`; private to that module.
const TOOL_CALL_THRESHOLD: usize = 3;

const SUBSTANTIAL_PROMPT: &str = "Implement complete hook parity. Do not regress tests. \
     Ensure every user-visible hook output matches Python.";

const ORIGINAL_REQUEST_HEADING: &str = "## 🎯 ORIGINAL USER REQUEST - PRESERVE THESE REQUIREMENTS";

const NO_ROOT_MESSAGE: &str =
    "Pre-compact state not saved: no amplihack state root (set HOME or AMPLIHACK_HOME)";

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// How `HOME` and `AMPLIHACK_HOME` are set for one test.
enum Root {
    /// `HOME=<home>`, `AMPLIHACK_HOME` unset: state root `<home>/.amplihack`.
    Home,
    /// `HOME=<home>`, `AMPLIHACK_HOME=<value>`.
    HomeAndAmplihackHome(PathBuf),
    /// Neither variable set: no state root resolves.
    None,
}

/// A throwaway home and project directory, with the cwd in the project.
///
/// Fields drop in declaration order: the cwd and environment are restored
/// before the temp directories are removed, and the lock is released last.
struct Fixture {
    _cwd: Option<CwdGuard>,
    _state: Option<IsolatedStateHome>,
    _env: Vec<EnvVarGuard>,
    project: tempfile::TempDir,
    home: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_root(Root::Home)
    }

    fn with_root(root: Root) -> Self {
        let lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let base = scratch_base::scratch_base();
        let home = tempfile::tempdir_in(&base).unwrap();
        let project = tempfile::tempdir_in(&base).unwrap();

        let mut env = vec![
            EnvVarGuard::unset("AMPLIHACK_RECIPE_RUN_ID"),
            EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH"),
            EnvVarGuard::unset("AMPLIHACK_ROOT"),
            EnvVarGuard::unset("AMPLIHACK_GRAPH_DB_PATH"),
            EnvVarGuard::unset("AMPLIHACK_KUZU_DB_PATH"),
            EnvVarGuard::set("AMPLIHACK_BLARIFY_MODE", "skip"),
            EnvVarGuard::set("AMPLIHACK_DISABLE_BLARIFY", "1"),
            EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude"),
        ];
        let state = match root {
            Root::Home => Some(isolate_state_home(home.path())),
            Root::HomeAndAmplihackHome(value) => {
                let state = isolate_state_home(home.path());
                env.push(EnvVarGuard::set("AMPLIHACK_HOME", value));
                Some(state)
            }
            Root::None => {
                env.push(EnvVarGuard::unset("HOME"));
                env.push(EnvVarGuard::unset("AMPLIHACK_HOME"));
                None
            }
        };
        let cwd = CwdGuard::set(project.path()).unwrap();

        Self {
            _cwd: Some(cwd),
            _state: state,
            _env: env,
            project,
            home,
            _lock: lock,
        }
    }

    /// Move the cwd somewhere else for the rest of the test.
    fn chdir(&mut self, dir: &Path) {
        // Restore the original cwd first so the guards nest correctly.
        self._cwd = None;
        self._cwd = Some(CwdGuard::set(dir).unwrap());
    }

    fn project(&self) -> &Path {
        self.project.path()
    }

    /// `<home>/.amplihack`: the state root under `Root::Home`.
    fn state_root(&self) -> PathBuf {
        self.home.path().join(".amplihack")
    }

    fn sessions(&self) -> PathBuf {
        self.state_root().join("runtime").join("sessions")
    }

    fn session(&self, key: &str) -> PathBuf {
        self.sessions().join(key)
    }

    fn projects(&self) -> PathBuf {
        self.state_root().join("runtime").join("projects")
    }
}

// ---------------------------------------------------------------------------
// Hook drivers
// ---------------------------------------------------------------------------

fn post_tool_use(tool: &str, input: Value, session: Option<&str>) -> Value {
    PostToolUseHook
        .process(HookInput::PostToolUse {
            tool_name: tool.to_string(),
            tool_input: input,
            tool_result: None,
            session_id: session.map(str::to_string),
        })
        .expect("post-tool-use never fails")
}

fn bash(session: Option<&str>) -> Value {
    post_tool_use("Bash", json!({"command": "ls"}), session)
}

fn edit_code(session: Option<&str>) -> Value {
    post_tool_use(
        "Edit",
        json!({"file_path": "src/main.rs", "old_string": "a", "new_string": "b"}),
        session,
    )
}

fn dev_skill(session: Option<&str>) -> Value {
    post_tool_use("Skill", json!({"skill": "dev-orchestrator"}), session)
}

fn view(session: Option<&str>) -> Value {
    post_tool_use("View", json!({"path": "/tmp/random.txt"}), session)
}

fn pre_compact(session: Option<&str>, transcript: Option<&Path>, extra: Value) -> Value {
    PreCompactHook
        .process(HookInput::PreCompact {
            session_id: session.map(str::to_string),
            transcript_path: transcript.map(Path::to_path_buf),
            extra,
        })
        .expect("pre-compact never returns Err")
}

fn session_start(session: Option<&str>, prompt: &str) -> Value {
    SessionStartHook
        .process(HookInput::SessionStart {
            session_id: session.map(str::to_string),
            cwd: None,
            extra: json!({ "prompt": prompt }),
        })
        .expect("session-start succeeds")
}

fn classify(session: Option<&str>, turn: Option<u64>, prompt: &str) -> Value {
    let extra = match turn {
        Some(t) => json!({ "turnCount": t }),
        None => json!({}),
    };
    WorkflowClassificationReminderHook
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some(prompt.to_string()),
            session_id: session.map(str::to_string),
            extra,
        })
        .expect("classification never fails on a prompt")
}

fn user_prompt(session: Option<&str>, prompt: &str) -> Value {
    UserPromptSubmitHook::with_prompt_root(|| None)
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some(prompt.to_string()),
            session_id: session.map(str::to_string),
            extra: Value::Null,
        })
        .expect("user-prompt-submit succeeds")
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

/// Every file and directory under `dir`, relative to it, with file contents.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap().to_path_buf();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.insert(rel, None);
                walk(base, &path, out);
            } else {
                out.insert(rel, Some(fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Every regular file under `dir`, as absolute paths.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    snapshot(dir)
        .into_iter()
        .filter(|(_, body)| body.is_some())
        .map(|(rel, _)| dir.join(rel))
        .collect()
}

fn assert_unchanged(dir: &Path, before: &BTreeMap<PathBuf, Option<Vec<u8>>>, what: &str) {
    let after = snapshot(dir);
    let added: Vec<_> = after.keys().filter(|k| !before.contains_key(*k)).collect();
    let changed: Vec<_> = after
        .iter()
        .filter(|(k, v)| before.get(*k).is_some_and(|b| b != *v))
        .map(|(k, _)| k)
        .collect();
    assert!(
        added.is_empty() && changed.is_empty() && after.len() == before.len(),
        "{what} wrote into {}: added {added:?}, changed {changed:?}",
        dir.display()
    );
}

/// No file whose name is one of the moved artefacts exists under `dir`.
fn assert_no_moved_artefacts(dir: &Path) {
    const MOVED: &[&str] = &[
        "post_tool_use_metrics.jsonl",
        "blarify_stale",
        "ORIGINAL_REQUEST.md",
        "original_request.json",
        "transcript_pre_compact.jsonl",
        "compaction_metadata.jsonl",
        "classification_state.json",
        "workflow_state.json",
    ];
    for file in files_under(dir) {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !MOVED.contains(&name.as_str()),
            "moved artefact {} found under {}",
            file.display(),
            dir.display()
        );
        let text = file.to_string_lossy();
        assert!(
            !text.contains("classification_state") && !text.contains("workflow_state"),
            "old state directory written: {}",
            file.display()
        );
    }
    assert!(
        !dir.join(".claude").join("runtime").exists(),
        "{}/.claude/runtime must not be created",
        dir.display()
    );
}

/// Every file under `state_root` lies in `runtime/sessions/<key>/` for a key
/// made of `[A-Za-z0-9_-]`, and returns the set of keys seen.
fn assert_all_under_sessions(state_root: &Path) -> Vec<String> {
    let sessions = state_root.join("runtime").join("sessions");
    let mut keys = Vec::new();
    for file in files_under(state_root) {
        let rel = file
            .strip_prefix(&sessions)
            .unwrap_or_else(|_| panic!("{} is outside {}", file.display(), sessions.display()));
        let mut components = rel.components();
        let Some(Component::Normal(key)) = components.next() else {
            panic!("{} has no session key component", file.display());
        };
        let key = key.to_str().expect("key is ASCII").to_string();
        assert!(
            !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "key {key:?} of {} is outside [A-Za-z0-9_-]",
            file.display()
        );
        assert!(
            rel.components().all(|c| matches!(c, Component::Normal(_))),
            "{} has a non-normal component",
            file.display()
        );
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// `<first 16 lowercase hex of sha256(canonicalised path bytes)>`.
fn project_key(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let digest = Sha256::digest(canonical.as_os_str().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn read_json(path: &Path) -> Value {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

fn jsonl(path: &Path) -> Vec<Value> {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("JSON line"))
        .collect()
}

fn additional_context(out: &Value) -> &str {
    out.pointer("/hookSpecificOutput/additionalContext")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no additionalContext in {out}"))
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("cannot stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ---------------------------------------------------------------------------
// DATA-1: a git checkout and the installed bundle stay unchanged
// ---------------------------------------------------------------------------

#[test]
fn git_checkout_stays_clean_after_post_tool_use() {
    let fx = Fixture::new();
    git(
        fx.project(),
        &["-c", "init.defaultBranch=main", "init", "-q"],
    );

    bash(Some("git-1"));
    edit_code(Some("git-1"));
    dev_skill(Some("git-1"));
    view(Some("git-1"));

    let status = git(
        fx.project(),
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignored",
        ],
    );
    assert_eq!(status, "", "post-tool-use dirtied the checkout:\n{status}");
    assert!(!fx.project().join(".claude").exists());
    assert!(!fx.project().join(".amplihack").exists());
}

#[test]
fn git_checkout_stays_clean_after_every_moved_writer() {
    let fx = Fixture::new();
    git(
        fx.project(),
        &["-c", "init.defaultBranch=main", "init", "-q"],
    );
    let transcript = fx.home.path().join("transcript.jsonl");
    fs::write(&transcript, "{\"role\":\"user\",\"content\":\"hello\"}\n").unwrap();

    bash(Some("git-2"));
    edit_code(Some("git-2"));
    dev_skill(Some("git-2"));
    pre_compact(
        Some("git-2"),
        Some(&transcript),
        json!({"conversation": [{"role": "user", "content": SUBSTANTIAL_PROMPT}]}),
    );
    session_start(Some("git-2"), SUBSTANTIAL_PROMPT);
    classify(Some("git-2"), None, "Build a login page for the admin area");

    let status = git(
        fx.project(),
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignored",
        ],
    );
    assert_eq!(status, "", "hooks dirtied the checkout:\n{status}");
    assert_no_moved_artefacts(fx.project());
    assert!(!fx.project().join(".amplihack").exists());
}

/// The installed bundle lives at `<state root>/.claude`. A session whose cwd
/// is the bundle, or its `recipes/` subdirectory, must not change it.
#[test]
fn installed_bundle_tree_is_unchanged() {
    let mut fx = Fixture::new();
    let bundle = fx.state_root().join(".claude");
    let recipes = bundle.join("recipes");
    fs::create_dir_all(&recipes).unwrap();
    fs::create_dir_all(bundle.join("tools/amplihack/hooks")).unwrap();
    fs::create_dir_all(bundle.join("context")).unwrap();
    fs::write(
        recipes.join("default-workflow.yaml"),
        "name: default-workflow\n",
    )
    .unwrap();
    fs::write(bundle.join("tools/amplihack/hooks/README.md"), "# hooks\n").unwrap();
    fs::write(bundle.join("context/PROJECT.md"), "# Bundle\n").unwrap();
    let transcript = fx.home.path().join("transcript.jsonl");
    fs::write(&transcript, "{\"role\":\"user\",\"content\":\"hello\"}\n").unwrap();

    let before = snapshot(&bundle);
    for (dir, sid) in [(bundle.clone(), "bundle-1"), (recipes.clone(), "bundle-2")] {
        fx.chdir(&dir);
        bash(Some(sid));
        edit_code(Some(sid));
        dev_skill(Some(sid));
        view(Some(sid));
        pre_compact(Some(sid), Some(&transcript), json!({}));
        session_start(Some(sid), SUBSTANTIAL_PROMPT);
        classify(Some(sid), None, "Build a login page for the admin area");
    }
    assert_unchanged(&bundle, &before, "hooks run inside the installed bundle");
    assert!(
        fx.session("bundle-1")
            .join("metrics/post_tool_use_metrics.jsonl")
            .exists(),
        "the metric lands in the state root instead"
    );
}

// ---------------------------------------------------------------------------
// Tool metrics
// ---------------------------------------------------------------------------

#[test]
fn metrics_land_in_session_state_dir_one_line_per_call() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    assert_eq!(bash(Some("metrics-1")), json!({}));
    assert_eq!(bash(Some("metrics-1")), json!({}));

    let file = fx
        .session("metrics-1")
        .join("metrics")
        .join("post_tool_use_metrics.jsonl");
    let lines = jsonl(&file);
    assert_eq!(
        lines.len(),
        2,
        "one JSON line per call in {}",
        file.display()
    );
    for line in &lines {
        assert_eq!(line["tool_name"], "Bash");
        assert_eq!(line["category"], "bash_commands");
        assert_eq!(line["session_id"], "metrics-1");
        assert_eq!(line["hook"], "post_tool_use");
    }
    assert_unchanged(fx.project(), &before, "the metric write");
}

#[test]
fn metrics_without_session_id_use_unknown_session_key() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    bash(None);

    let file = fx
        .session("unknown-session")
        .join("metrics/post_tool_use_metrics.jsonl");
    let lines = jsonl(&file);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].get("session_id").is_none());
    assert_unchanged(fx.project(), &before, "the metric write");
}

// ---------------------------------------------------------------------------
// VAL-2/VAL-3: hostile and over-long session ids
// ---------------------------------------------------------------------------

#[test]
fn hostile_session_ids_stay_inside_sessions_dir() {
    let cases: &[(&str, &str)] = &[
        ("../../escape", "______escape"),
        ("/abs", "_abs"),
        ("a/b", "a_b"),
        ("..", "__"),
        ("x\0y", "x_y"),
        ("", "unknown-session"),
    ];
    for (id, key) in cases {
        let fx = Fixture::new();
        let transcript = fx.project().join("t.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        let before_with_transcript = snapshot(fx.project());

        bash(Some(id));
        let out = pre_compact(Some(id), Some(&transcript), json!({}));
        assert_eq!(
            out["hookSpecificOutput"]["status"], "success",
            "id {id:?}: {out}"
        );
        classify(Some(id), None, "Build a login page for the admin area");

        let keys = assert_all_under_sessions(&fx.state_root());
        assert_eq!(
            keys,
            vec![key.to_string()],
            "id {id:?} wrote under {keys:?}"
        );
        assert!(
            fx.session(key)
                .join("metrics/post_tool_use_metrics.jsonl")
                .exists(),
            "id {id:?}: metric not under key {key:?}"
        );
        assert!(
            fx.session(key)
                .join("logs/transcript_pre_compact.jsonl")
                .exists(),
            "id {id:?}: transcript not under key {key:?}"
        );
        assert!(
            !fx.state_root().join("runtime").join("escape").exists()
                && !fx.home.path().join("escape").exists()
                && !Path::new("/abs").join("metrics").exists(),
            "id {id:?} escaped the sessions directory"
        );
        assert_unchanged(fx.project(), &before_with_transcript, "a hostile id");
    }
}

/// An id longer than `NAME_MAX` makes the write fail. The hook logs it,
/// writes nothing elsewhere and still returns its usual output.
#[test]
fn long_session_id_fails_open_without_writing_elsewhere() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());
    let long = "a".repeat(300);

    assert_eq!(bash(Some(&long)), json!({}));
    assert_eq!(edit_code(Some(&long)), json!({}));
    let out = classify(Some(&long), None, "Build a login page for the admin area");
    assert_eq!(out, json!({}), "a failed state write holds the prompt back");

    assert_unchanged(fx.project(), &before, "an over-long id");
    for file in files_under(&fx.state_root()) {
        assert!(
            file.starts_with(fx.projects()),
            "only the project-keyed blarify marker may be written: {}",
            file.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Resolving the state root from AMPLIHACK_HOME and HOME
// ---------------------------------------------------------------------------

#[test]
fn absolute_amplihack_home_is_used_as_state_root() {
    let base = scratch_base::scratch_base();
    let amp = tempfile::tempdir_in(&base).unwrap();
    let fx = Fixture::with_root(Root::HomeAndAmplihackHome(amp.path().to_path_buf()));
    let before = snapshot(fx.project());

    bash(Some("amp-1"));

    assert!(
        amp.path()
            .join("runtime/sessions/amp-1/metrics/post_tool_use_metrics.jsonl")
            .exists()
    );
    assert!(
        !fx.state_root().exists(),
        "$HOME/.amplihack must not be used when AMPLIHACK_HOME is absolute"
    );
    assert_unchanged(fx.project(), &before, "the metric write");
}

#[test]
fn relative_amplihack_home_falls_back_to_home_dot_amplihack() {
    let fx = Fixture::with_root(Root::HomeAndAmplihackHome(PathBuf::from("relative/amp")));
    let before = snapshot(fx.project());

    bash(Some("rel-1"));

    assert!(
        fx.session("rel-1")
            .join("metrics/post_tool_use_metrics.jsonl")
            .exists()
    );
    assert!(!fx.project().join("relative").exists());
    assert_unchanged(fx.project(), &before, "the metric write");
}

#[test]
fn amplihack_home_with_parent_component_falls_back_to_home_dot_amplihack() {
    let fx = Fixture::with_root(Root::HomeAndAmplihackHome(parent_component_path()));
    let before = snapshot(fx.project());

    bash(Some("dotdot-1"));

    assert!(
        fx.session("dotdot-1")
            .join("metrics/post_tool_use_metrics.jsonl")
            .exists()
    );
    assert_unchanged(fx.project(), &before, "the metric write");
}

/// #2361: an `AMPLIHACK_HOME` that is a Git checkout (the `-w` checkout of a
/// recipe run) is skipped; the state goes to `$HOME/.amplihack` and the
/// checkout's index and working tree are unchanged.
#[test]
fn checkout_amplihack_home_falls_through_to_home_dot_amplihack() {
    let base = scratch_base::scratch_base();
    let checkout = tempfile::tempdir_in(&base).unwrap();
    git(
        checkout.path(),
        &["-c", "init.defaultBranch=main", "init", "-q"],
    );
    let fx = Fixture::with_root(Root::HomeAndAmplihackHome(checkout.path().to_path_buf()));
    let before = snapshot(checkout.path());

    bash(Some("checkout-1"));

    assert!(
        fx.session("checkout-1")
            .join("metrics/post_tool_use_metrics.jsonl")
            .exists(),
        "the metric must land under $HOME/.amplihack"
    );
    assert!(!checkout.path().join("runtime").exists());
    assert_unchanged(checkout.path(), &before, "the metric write");
    let status = git(
        checkout.path(),
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
    );
    assert_eq!(status, "", "post-tool-use dirtied the checkout:\n{status}");
}

fn parent_component_path() -> PathBuf {
    scratch_base::scratch_base()
        .join("x")
        .join("..")
        .join("amp-dotdot")
}

#[test]
fn unset_amplihack_home_uses_home_dot_amplihack() {
    let fx = Fixture::new();
    bash(Some("unset-1"));
    assert!(
        fx.home
            .path()
            .join(".amplihack/runtime/sessions/unset-1/metrics/post_tool_use_metrics.jsonl")
            .exists()
    );
}

// ---------------------------------------------------------------------------
// AUTH-1/R4/R5: no state root resolves
// ---------------------------------------------------------------------------

#[test]
fn no_root_post_tool_use_writes_nothing_and_succeeds() {
    let fx = Fixture::with_root(Root::None);
    let before = snapshot(fx.project());

    assert_eq!(bash(Some("noroot-1")), json!({}));
    assert_eq!(edit_code(Some("noroot-1")), json!({}));
    assert_eq!(dev_skill(Some("noroot-1")), json!({}));
    for _ in 0..TOOL_CALL_THRESHOLD + 1 {
        assert_eq!(view(Some("noroot-1")), json!({}));
    }

    assert_unchanged(fx.project(), &before, "post-tool-use with no state root");
    assert!(!fx.state_root().exists());
}

#[test]
fn no_root_pre_compact_returns_exact_success_shape() {
    let fx = Fixture::with_root(Root::None);
    let transcript = fx.project().join("input.jsonl");
    fs::write(&transcript, "{\"role\":\"user\",\"content\":\"hello\"}\n").unwrap();
    let before = snapshot(fx.project());

    let out = pre_compact(
        Some("noroot-pc"),
        Some(&transcript),
        json!({"conversation": [{"role": "user", "content": SUBSTANTIAL_PROMPT}]}),
    );

    assert_eq!(
        out,
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreCompact",
                "status": "success",
                "message": NO_ROOT_MESSAGE,
                "transcript_path": null,
                "metadata": null,
            }
        })
    );
    let object = out["hookSpecificOutput"].as_object().unwrap();
    assert!(object.contains_key("transcript_path") && object.contains_key("metadata"));
    assert_unchanged(fx.project(), &before, "pre-compact with no state root");
}

#[test]
fn no_root_pre_compact_without_session_id_returns_same_shape() {
    let fx = Fixture::with_root(Root::None);
    let before = snapshot(fx.project());

    let out = pre_compact(None, None, json!({}));

    assert_eq!(out["hookSpecificOutput"]["status"], "success");
    assert_eq!(out["hookSpecificOutput"]["message"], NO_ROOT_MESSAGE);
    assert!(out["hookSpecificOutput"]["transcript_path"].is_null());
    assert!(out["hookSpecificOutput"]["metadata"].is_null());
    assert_unchanged(fx.project(), &before, "pre-compact with no state root");
}

#[test]
fn no_root_session_start_still_injects_original_request_context() {
    let fx = Fixture::with_root(Root::None);

    let out = session_start(Some("noroot-ss"), SUBSTANTIAL_PROMPT);

    let context = additional_context(&out);
    assert!(context.contains(ORIGINAL_REQUEST_HEADING), "{context}");
    assert!(context.contains("**Constraints**:"));
    assert_no_moved_artefacts(fx.project());
}

#[test]
fn no_root_classification_holds_back_a_new_topic() {
    let fx = Fixture::with_root(Root::None);
    let before = snapshot(fx.project());

    let reported = classify(
        Some("noroot-c"),
        Some(0),
        "Build a login page for the admin area",
    );
    assert_eq!(reported, json!({}), "R4: a new topic gets held_back_output");

    let counted = classify(
        Some("noroot-c"),
        None,
        "Build a login page for the admin area",
    );
    assert_eq!(counted, json!({}), "R4: a new topic gets held_back_output");

    assert_unchanged(fx.project(), &before, "classification with no state root");
}

#[test]
fn no_root_classification_marked_prompt_gets_the_note() {
    let fx = Fixture::with_root(Root::None);
    let before = snapshot(fx.project());

    let out = classify(Some("noroot-m"), Some(0), "[coord] report your lane status");
    assert!(
        additional_context(&out).starts_with("[coordination]"),
        "a marked prompt keeps the coordination note: {out}"
    );
    assert_unchanged(fx.project(), &before, "classification with no state root");
}

#[test]
fn no_root_dev_prompt_is_still_processed() {
    let fx = Fixture::with_root(Root::None);

    let out = user_prompt(Some("noroot-dev"), "/dev continue parity audit");

    assert!(
        additional_context(&out).contains("/dev workflow detected"),
        "{out}"
    );
    assert_no_moved_artefacts(fx.project());
}

// ---------------------------------------------------------------------------
// Workflow enforcement state
// ---------------------------------------------------------------------------

#[test]
fn workflow_state_lands_in_session_state_dir_and_round_trips() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    assert_eq!(dev_skill(Some("wf-1")), json!({}));
    let state_file = fx.session("wf-1").join("workflow_state.json");
    let state = read_json(&state_file);
    assert_eq!(state["tool_calls_since"], 0);
    assert_eq!(state["warning_emitted"], false);

    for _ in 0..TOOL_CALL_THRESHOLD - 1 {
        assert!(view(Some("wf-1")).get("warnings").is_none());
    }
    assert_eq!(
        read_json(&state_file)["tool_calls_since"],
        (TOOL_CALL_THRESHOLD - 1) as u64
    );
    let warned = view(Some("wf-1"));
    assert!(
        warned["warnings"][0]
            .as_str()
            .unwrap()
            .contains("WORKFLOW BYPASS DETECTED"),
        "{warned}"
    );
    assert_eq!(read_json(&state_file)["warning_emitted"], true);

    post_tool_use(
        "Bash",
        json!({"command": "amplihack recipe run smart-orchestrator"}),
        Some("wf-1"),
    );
    assert!(!state_file.exists(), "workflow evidence clears the state");

    assert_unchanged(fx.project(), &before, "workflow enforcement");
}

/// In one id-less PostToolUse call the workflow state goes under `current`
/// and the metric under `unknown-session`. This is intended.
#[test]
fn workflow_state_without_session_id_uses_current_key() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    dev_skill(None);

    assert!(fx.session("current").join("workflow_state.json").exists());
    assert!(
        fx.session("unknown-session")
            .join("metrics/post_tool_use_metrics.jsonl")
            .exists()
    );
    assert!(
        !fx.session("unknown-session")
            .join("workflow_state.json")
            .exists()
    );
    assert!(!fx.session("current").join("metrics").exists());

    for _ in 0..TOOL_CALL_THRESHOLD - 1 {
        assert!(view(None).get("warnings").is_none());
    }
    assert!(
        view(None)["warnings"][0]
            .as_str()
            .unwrap()
            .contains("WORKFLOW BYPASS DETECTED")
    );

    assert_unchanged(fx.project(), &before, "workflow enforcement");
}

#[test]
fn dev_prompt_starts_tracking_under_the_state_root() {
    let fx = Fixture::new();

    let out = user_prompt(Some("wf-dev"), "/dev continue parity audit");
    assert!(additional_context(&out).contains("/dev workflow detected"));

    let state_file = fx.session("wf-dev").join("workflow_state.json");
    assert_eq!(read_json(&state_file)["tool_calls_since"], 0);
    for _ in 0..TOOL_CALL_THRESHOLD - 1 {
        assert!(view(Some("wf-dev")).get("warnings").is_none());
    }
    assert!(
        view(Some("wf-dev"))["warnings"][0]
            .as_str()
            .unwrap()
            .contains("WORKFLOW BYPASS DETECTED")
    );
    assert_no_moved_artefacts(fx.project());
}

#[test]
fn id_less_dev_prompt_and_post_tool_use_share_current_state() {
    let fx = Fixture::new();

    user_prompt(None, "/dev continue parity audit");
    assert!(fx.session("current").join("workflow_state.json").exists());

    for _ in 0..TOOL_CALL_THRESHOLD - 1 {
        assert!(view(None).get("warnings").is_none());
    }
    assert!(
        view(None)["warnings"][0]
            .as_str()
            .unwrap()
            .contains("WORKFLOW BYPASS DETECTED")
    );
    assert_no_moved_artefacts(fx.project());
}

// ---------------------------------------------------------------------------
// Blarify staleness marker (VAL-6)
// ---------------------------------------------------------------------------

#[test]
fn blarify_marker_lands_in_project_keyed_state_dir() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    edit_code(Some("b-1"));

    let key = project_key(fx.project());
    let marker = fx.projects().join(&key).join("blarify_stale");
    let body = read_json(&marker);
    assert_eq!(body["stale"], true);
    assert_eq!(body["reason"], "code_file_modified");
    assert_eq!(body["tool"], "Edit");
    assert_eq!(body["path"], "src/main.rs");
    assert!(body["timestamp"].as_u64().is_some());

    assert!(!fx.project().join(".amplihack").exists());
    assert_unchanged(fx.project(), &before, "the blarify marker");
}

#[test]
fn blarify_marker_key_is_stable_and_uses_canonical_cwd() {
    let mut fx = Fixture::new();
    let link = fx.home.path().join("project-link");
    std::os::unix::fs::symlink(fx.project(), &link).unwrap();

    edit_code(Some("b-2"));
    fx.chdir(&link);
    post_tool_use(
        "MultiEdit",
        json!({"edits": [{"file_path": "lib/a.py"}]}),
        Some("b-2"),
    );

    let entries: Vec<_> = fs::read_dir(fx.projects())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        entries,
        vec![project_key(fx.project())],
        "both runs share one project key"
    );
    let key = &entries[0];
    assert_eq!(key.len(), 16);
    assert!(
        key.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    let project_name = fx
        .project()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(
        !key.contains(&project_name),
        "the project path never appears in a path component"
    );
    let body = read_json(&fx.projects().join(key).join("blarify_stale"));
    assert_eq!(
        body["path"], "lib/a.py",
        "the second run overwrites the marker"
    );
}

#[test]
fn blarify_marker_not_written_for_non_code_file() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    post_tool_use(
        "Edit",
        json!({"file_path": "docs/README.md", "old_string": "a", "new_string": "b"}),
        Some("b-3"),
    );

    assert!(!fx.projects().exists());
    assert_unchanged(fx.project(), &before, "a non-code edit");
}

// ---------------------------------------------------------------------------
// Original request (session start)
// ---------------------------------------------------------------------------

#[test]
fn session_start_saves_original_request_under_session_logs() {
    let fx = Fixture::new();

    let out = session_start(Some("ss-1"), SUBSTANTIAL_PROMPT);

    assert!(additional_context(&out).contains(ORIGINAL_REQUEST_HEADING));
    let logs = fx.session("ss-1").join("logs");
    assert!(logs.join("ORIGINAL_REQUEST.md").exists());
    let record = read_json(&logs.join("original_request.json"));
    assert_eq!(record["session_id"], "ss-1");
    assert_eq!(record["raw_prompt"], SUBSTANTIAL_PROMPT);
    assert_no_moved_artefacts(fx.project());
}

/// R3: the directory comes from the session id the hook received, never from
/// the record's `session-start` fallback.
#[test]
fn session_start_without_id_uses_unknown_session_dir_but_keeps_record_id() {
    let fx = Fixture::new();

    session_start(None, SUBSTANTIAL_PROMPT);

    let logs = fx.session("unknown-session").join("logs");
    assert!(logs.join("ORIGINAL_REQUEST.md").exists());
    let record = read_json(&logs.join("original_request.json"));
    assert_eq!(record["session_id"], "session-start");
    assert!(
        !fx.session("session-start").exists(),
        "the record's session_id must not choose the directory"
    );
    assert_no_moved_artefacts(fx.project());
}

#[test]
fn session_start_save_failure_keeps_the_context() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.sessions()).unwrap();
    // A regular file where the session directory should be makes the save fail.
    fs::write(fx.session("ss-fail"), "not a directory").unwrap();

    let out = session_start(Some("ss-fail"), SUBSTANTIAL_PROMPT);

    assert!(additional_context(&out).contains(ORIGINAL_REQUEST_HEADING));
    assert_eq!(
        fs::read_to_string(fx.session("ss-fail")).unwrap(),
        "not a directory"
    );
    assert_no_moved_artefacts(fx.project());
}

#[test]
fn session_start_short_prompt_writes_nothing() {
    let fx = Fixture::new();

    session_start(Some("ss-short"), "hi");

    assert!(!fx.session("ss-short").join("logs").exists());
    assert_no_moved_artefacts(fx.project());
}

// ---------------------------------------------------------------------------
// Pre-compact: transcript export, compaction metadata, original request
// ---------------------------------------------------------------------------

#[test]
fn pre_compact_exports_transcript_and_metadata_under_session_logs() {
    let fx = Fixture::new();
    let transcript = fx.project().join("input.jsonl");
    let body = "{\"type\":\"user\",\"message\":\"hello\"}\n";
    fs::write(&transcript, body).unwrap();
    let before = snapshot(fx.project());

    let out = pre_compact(Some("pc-1"), Some(&transcript), json!({"trigger": "auto"}));

    let output = &out["hookSpecificOutput"];
    assert_eq!(output["status"], "success", "{out}");
    let logs = fx.session("pc-1").join("logs");
    let exported = logs.join("transcript_pre_compact.jsonl");
    assert_eq!(
        output["transcript_path"].as_str(),
        Some(exported.to_string_lossy().as_ref()),
        "transcript_path reports the new location"
    );
    assert_eq!(fs::read_to_string(&exported).unwrap(), body);
    assert!(
        output["message"]
            .as_str()
            .unwrap()
            .contains(exported.to_string_lossy().as_ref())
    );
    assert_eq!(
        output["metadata"]["transcript_path"].as_str(),
        Some(exported.to_string_lossy().as_ref())
    );
    assert_eq!(output["metadata"]["compaction_trigger"], "auto");

    let lines = jsonl(&logs.join("compaction_metadata.jsonl"));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "pre_compact");
    assert_eq!(lines[0]["session_id"], "pc-1");

    assert_unchanged(fx.project(), &before, "pre-compact");
}

#[test]
fn pre_compact_saves_original_request_under_session_logs() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    let out = pre_compact(
        Some("pc-2"),
        None,
        json!({
            "trigger": "token_limit",
            "conversation": [{"role": "user", "content": SUBSTANTIAL_PROMPT}]
        }),
    );

    assert_eq!(out["hookSpecificOutput"]["status"], "success");
    assert_eq!(
        out["hookSpecificOutput"]["metadata"]["original_request_preserved"],
        true
    );
    let logs = fx.session("pc-2").join("logs");
    assert!(logs.join("ORIGINAL_REQUEST.md").exists());
    assert_eq!(
        read_json(&logs.join("original_request.json"))["session_id"],
        "pc-2"
    );
    assert!(logs.join("compaction_metadata.jsonl").exists());
    assert_unchanged(fx.project(), &before, "pre-compact");
}

#[test]
fn pre_compact_original_request_from_transcript_lands_under_session_logs() {
    let fx = Fixture::new();
    let transcript = fx.project().join("copilot-events.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({"type": "user.message", "data": {"content": SUBSTANTIAL_PROMPT}})
        ),
    )
    .unwrap();
    let before = snapshot(fx.project());

    let out = pre_compact(Some("pc-3"), Some(&transcript), Value::Null);

    assert_eq!(
        out["hookSpecificOutput"]["metadata"]["original_request_preserved"],
        true
    );
    assert!(fx.session("pc-3").join("logs/ORIGINAL_REQUEST.md").exists());
    assert_unchanged(fx.project(), &before, "pre-compact");
}

#[test]
fn pre_compact_without_session_id_uses_generated_key() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    let out = pre_compact(None, None, json!({}));

    assert_eq!(out["hookSpecificOutput"]["status"], "success");
    let keys = assert_all_under_sessions(&fx.state_root());
    assert_eq!(keys.len(), 1, "{keys:?}");
    let key = &keys[0];
    assert!(
        key.strip_prefix("session-")
            .is_some_and(|secs| secs.chars().all(|c| c.is_ascii_digit())),
        "generated key {key:?}"
    );
    assert!(
        fx.session(key)
            .join("logs/compaction_metadata.jsonl")
            .exists()
    );
    assert_unchanged(fx.project(), &before, "pre-compact");
}

/// Only the no-root case reports success with null fields. A failed write
/// under a resolved root is still an error, as before.
#[test]
fn pre_compact_write_failure_under_resolved_root_is_an_error() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.sessions()).unwrap();
    fs::write(fx.session("pc-fail"), "not a directory").unwrap();
    let transcript = fx.project().join("input.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let before = snapshot(fx.project());

    let with_transcript = pre_compact(Some("pc-fail"), Some(&transcript), json!({}));
    assert_eq!(
        with_transcript["hookSpecificOutput"]["status"], "error",
        "{with_transcript}"
    );

    let metadata_only = pre_compact(Some("pc-fail"), None, json!({}));
    assert_eq!(
        metadata_only["hookSpecificOutput"]["status"], "error",
        "{metadata_only}"
    );

    assert_unchanged(fx.project(), &before, "a failed pre-compact");
}

// ---------------------------------------------------------------------------
// Classification state
// ---------------------------------------------------------------------------

#[test]
fn classification_state_lands_in_session_state_dir() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    let out = classify(
        Some("c-1"),
        Some(0),
        "Build a login page for the admin area",
    );
    assert!(
        out.pointer("/hookSpecificOutput/additionalContext")
            .is_some(),
        "a new topic with a writable state root is routed: {out}"
    );

    let state = read_json(&fx.session("c-1").join("classification_state.json"));
    assert_eq!(state["last_rule"], "classification");
    assert_eq!(state["last_classified_turn"], 0);
    assert_eq!(state["session_id"], "c-1");
    assert_unchanged(fx.project(), &before, "classification");
}

#[test]
fn classification_turn_counter_persists_under_session_state_dir() {
    let fx = Fixture::new();
    let before = snapshot(fx.project());

    classify(Some("c-2"), None, "Build a login page for the admin area");
    classify(Some("c-2"), None, "Also add a logout button");

    let state = read_json(&fx.session("c-2").join("classification_state.json"));
    assert_eq!(state["observed_turns"], 2);
    assert_unchanged(fx.project(), &before, "classification");
}

#[test]
fn classification_without_session_id_uses_unknown_session() {
    let fx = Fixture::new();

    classify(None, None, "Build a login page for the admin area");

    let state = read_json(
        &fx.session("unknown-session")
            .join("classification_state.json"),
    );
    assert_eq!(state["observed_turns"], 1);
    assert_no_moved_artefacts(fx.project());
}

/// The throttle reads the classification it wrote under the state root.
#[test]
fn classification_throttle_reads_state_root() {
    let fx = Fixture::new();

    let first = classify(
        Some("c-3"),
        Some(4),
        "Build a login page for the admin area",
    );
    assert!(
        first
            .pointer("/hookSpecificOutput/additionalContext")
            .is_some()
    );
    let second = classify(Some("c-3"), Some(5), "Add a password reset form as well");
    assert_eq!(
        second,
        json!({}),
        "within three turns the prompt is throttled"
    );

    let state = read_json(&fx.session("c-3").join("classification_state.json"));
    assert_eq!(state["last_rule"], "throttled");
    assert_eq!(state["last_classified_turn"], 4);
    assert_no_moved_artefacts(fx.project());
}

// ---------------------------------------------------------------------------
// HARD-1: directories created 0700
// ---------------------------------------------------------------------------

#[test]
fn state_directories_are_created_private() {
    let fx = Fixture::new();
    bash(Some("perm-1"));
    edit_code(Some("perm-1"));
    pre_compact(
        Some("perm-1"),
        None,
        json!({"conversation": [{"role": "user", "content": SUBSTANTIAL_PROMPT}]}),
    );

    let key = project_key(fx.project());
    for dir in [
        fx.state_root(),
        fx.state_root().join("runtime"),
        fx.sessions(),
        fx.session("perm-1"),
        fx.session("perm-1").join("metrics"),
        fx.session("perm-1").join("logs"),
        fx.projects(),
        fx.projects().join(&key),
    ] {
        assert_eq!(
            mode(&dir),
            0o700,
            "{} must be created with mode 0700",
            dir.display()
        );
    }
}

#[test]
fn existing_state_root_keeps_its_mode() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.state_root()).unwrap();
    fs::set_permissions(fx.state_root(), fs::Permissions::from_mode(0o755)).unwrap();

    bash(Some("perm-2"));

    assert_eq!(
        mode(&fx.state_root()),
        0o755,
        "existing directories keep their mode"
    );
    assert_eq!(mode(&fx.state_root().join("runtime")), 0o700);
    assert_eq!(mode(&fx.session("perm-2")), 0o700);
}
