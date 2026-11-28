//! Issue #3435: guidance appended while an agent step runs reaches that step
//! at its next tool call, once; a step whose launch form has no hook is named
//! to the sender at once.
//!
//! Real `amplihack` launcher, real `amplihack-hooks` binary. The stand-in
//! `claude` and `codex` executables find amplihack's PostToolUse handler the
//! way their harness does (Claude from the `--settings` file the launcher
//! passes, Codex from `$CODEX_HOME/hooks.json`), then block on a FIFO. The
//! test appends guidance with `amplihack recipe guide` while the step is
//! blocked, then releases it, and the stand-in runs the handler twice with a
//! payload in its harness's shape, saving each output. There is no sleep,
//! poll or timer: the FIFO open is the rendezvous.
//!
//! The assertions read the hook output's added context, the records in
//! `guidance.log` and whether `recipe guide` names the step id. None
//! compares the wording of a message.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::fifo_release;
use crate::support::recipe_guide::{GuideFixture, show, stdout, written_lines};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MARKER: &str = "MARKER-3435-check-the-release-notes-before-you-push";

/// Runs the PostToolUse handler named in `$HOOKS_FILE` (a settings or
/// hooks file) once before blocking, as a step's first tool call, saving its
/// output in `hook.0`, then twice after the release, in `hook.1` and
/// `hook.2`. With `HOOKS_NEVER_FIRE` set it runs none, as a harness whose
/// hook does not fire.
const RUN_HOOKS: &str = r#"
printf '%s' "$AMPLIHACK_GUIDANCE_STEP_ID" > "$RECORD_DIR/stepid"
printf '%s' "$HOOKS_FILE" > "$RECORD_DIR/hooksfile"
cmd=$(grep -o '"command": *".* post-tool-use"' "$HOOKS_FILE" | head -n 1 | sed 's/^"command": *"//; s/"$//; s/\\"/"/g')
[ -n "$HOOKS_NEVER_FIRE" ] && cmd=
if [ -n "$cmd" ]; then
  printf '%s' "$HOOK_PAYLOAD" | sh -c "$cmd" > "$RECORD_DIR/hook.0"
fi
read _ < "$RELEASE_FIFO"
if [ -n "$cmd" ]; then
  for n in 1 2; do
    printf '%s' "$HOOK_PAYLOAD" | sh -c "$cmd" > "$RECORD_DIR/hook.$n"
  done
fi
exit 0
"#;

/// Claude Code: the hooks file is the last `--settings` argument.
fn standin_claude() -> String {
    format!(
        r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
cat > "$RECORD_DIR/prompt"
HOOKS_FILE=
prev=
for a in "$@"; do
  [ "$prev" = "--settings" ] && HOOKS_FILE=$a
  prev=$a
done
HOOK_PAYLOAD='{{"hook_event_name":"PostToolUse","session_id":"s-3435","tool_name":"Bash","tool_input":{{"command":"true"}},"tool_response":{{"stdout":""}}}}'
{RUN_HOOKS}"#
    )
}

/// Codex: reads its prompt from stdin; the hooks file is
/// `$CODEX_HOME/hooks.json`.
fn standin_codex() -> String {
    format!(
        r#"#!/bin/sh
case "$1" in
  --version) echo 'codex-cli 0.166.0'; exit 0 ;;
  plugin)
    case "$2" in
      list) echo '{{"installed":[]}}' ;;
      *) echo '{{"ok":true}}' ;;
    esac
    exit 0 ;;
esac
cat > "$RECORD_DIR/prompt"
HOOKS_FILE="$CODEX_HOME/hooks.json"
HOOK_PAYLOAD='{{"hook_event_name":"PostToolUse","session_id":"s-3435","tool_name":"shell","tool_input":{{"command":["true"]}},"tool_response":"ok"}}'
{RUN_HOOKS}"#
    )
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn hooks_binary() -> PathBuf {
    let hooks = amplihack_bin().with_file_name("amplihack-hooks");
    assert!(hooks.is_file(), "no amplihack-hooks beside amplihack");
    hooks
}

struct Step {
    f: GuideFixture,
    guidance: PathBuf,
    live: fifo_release::Live,
}

impl Step {
    /// The step id the launcher gave the agent.
    fn step_id(&self) -> String {
        self.f.record("stepid")
    }

    fn guide(&self) -> std::process::Output {
        let o = self.f.guide_text(&["r-3435"], MARKER);
        assert!(o.status.success(), "{}", show(&o));
        o
    }

    fn entry_name(o: &std::process::Output) -> String {
        let written = written_lines(o);
        assert_eq!(written.len(), 1, "{}", show(o));
        written[0]
            .0
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    /// The added context of hook output `n`, empty when it has none.
    fn context(&self, n: u32) -> String {
        let Some(raw) = self.f.record_opt(&format!("hook.{n}")) else {
            let file = self.f.record_opt("hooksfile").unwrap_or_default();
            let body = fs::read_to_string(&file).unwrap_or_default();
            panic!("no output from hook {n}; hooks file {file:?} holds:\n{body}");
        };
        let value: serde_json::Value =
            serde_json::from_str(raw.trim()).unwrap_or_else(|e| panic!("{e}: {raw:?}"));
        value["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn records(&self, event: &str) -> Vec<serde_json::Value> {
        self.f
            .log(&self.guidance)
            .into_iter()
            .filter(|l| l["event"] == event)
            .collect()
    }
}

/// Starts `tail` through `amplihack <tool> --` in a run folder of a new
/// fixture, and returns once the stand-in is blocked on its FIFO.
fn start(tool: &str, standin: &str, tail: &[&str], stdin: Option<&[u8]>) -> Step {
    start_in(GuideFixture::new(), tool, standin, tail, stdin)
}

/// The step was given the entry at its first tool call after the append,
/// not at its second, and one `guidance_injected` record says so.
fn assert_delivered_once(step: &mut Step) {
    let o = step.guide();
    let entry = Step::entry_name(&o);
    let id = step.step_id();
    assert!(!id.is_empty());
    // A step whose hook has run is neither reported as unreachable nor as
    // unconfirmed.
    assert!(!stdout(&o).contains(&id), "{}", show(&o));
    assert!(step.context(0).is_empty(), "{}", step.context(0));

    let run = step.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));
    assert!(step.context(1).contains(MARKER), "{}", step.context(1));
    assert!(!step.context(2).contains(MARKER), "{}", step.context(2));

    let injected = step.records("guidance_injected");
    assert_eq!(injected.len(), 1, "{injected:?}");
    assert_eq!(injected[0]["step_id"], id.as_str());
    assert_eq!(injected[0]["entry_names"], serde_json::json!([entry]));
    assert!(step.records("guidance_dropped").is_empty());
    assert!(step.records("guidance_step_unhooked").is_empty());
    let hooked = step.records("guidance_step_hooked");
    assert_eq!(hooked.len(), 1, "{hooked:?}");
    assert_eq!(hooked[0]["step_id"], id.as_str());
    // Recorded once, by the first hook run, however many runs follow.
    let seen = step.records("guidance_step_hook_seen");
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["step_id"], id.as_str());
    // The guidance text is in the agent's context only.
    let raw = fs::read_to_string(step.guidance.parent().unwrap().join("guidance.log")).unwrap();
    assert!(!raw.contains(MARKER));
}

#[test]
fn a_claude_interactive_step_gets_guidance_appended_while_it_runs_at_its_next_tool_call() {
    let mut step = start("claude", &standin_claude(), &["--add-dir", "."], None);
    assert_delivered_once(&mut step);
}

#[test]
fn a_claude_print_step_gets_guidance_appended_while_it_runs_at_its_next_tool_call() {
    let mut step = start(
        "claude",
        &standin_claude(),
        &["-p", "--add-dir", "."],
        Some(b"print step prompt\n"),
    );
    assert_delivered_once(&mut step);
}

#[test]
fn guidance_given_at_launch_is_not_given_again_by_the_steps_hooks() {
    const EARLIER: &str = "MARKER-3435-earlier-entry-given-in-the-prompt";
    let f = GuideFixture::new();
    // `start_in` makes the run folder; make it first so the entry exists
    // before the step starts.
    f.make_run("r-3435", "c-3435");
    let before = f.guide_text(&["r-3435"], EARLIER);
    assert!(before.status.success(), "{}", show(&before));
    let earlier = Step::entry_name(&before);
    let mut step = start_in(
        f,
        "claude",
        &standin_claude(),
        &["-p", "--add-dir", "."],
        Some(b"print step prompt\n"),
    );
    let later = Step::entry_name(&step.guide());
    let id = step.step_id();
    let run = step.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));

    let prompt = step.f.record("prompt");
    assert!(prompt.starts_with("print step prompt\n"), "{prompt:?}");
    assert!(
        prompt.contains(EARLIER) && !prompt.contains(MARKER),
        "{prompt:?}"
    );
    assert!(step.context(1).contains(MARKER), "{}", step.context(1));
    assert!(!step.context(1).contains(EARLIER), "{}", step.context(1));
    assert!(step.context(2).is_empty(), "{}", step.context(2));

    let injected = step.records("guidance_injected");
    let names: Vec<&serde_json::Value> = injected.iter().map(|r| &r["entry_names"]).collect();
    assert_eq!(
        names,
        [&serde_json::json!([earlier]), &serde_json::json!([later])],
        "{injected:?}"
    );
    assert!(injected.iter().all(|r| r["step_id"] == id.as_str()));
}

#[test]
fn a_codex_exec_step_gets_guidance_appended_while_it_runs_at_its_next_tool_call() {
    let mut step = start_codex(&standin_codex());
    assert_delivered_once(&mut step);
    assert_eq!(step.f.record("prompt"), "codex prompt\n");
}

/// [`start`] for Codex, with the plugin's hook wrapper (as `amplihack
/// install` writes it, in a `bin/` folder) named for PostToolUse and
/// UserPromptSubmit in the fixture's `$CODEX_HOME/hooks.json`.
fn start_codex(standin: &str) -> Step {
    let f = GuideFixture::new();
    let plugin_bin = f.root.join("plugin").join("bin");
    fs::create_dir_all(&plugin_bin).unwrap();
    let wrapper = plugin_bin.join("hook");
    executable(
        &wrapper,
        &format!(
            "#!/bin/sh\nexport AMPLIHACK_HOOK_PROTOCOL=codex\nexec '{}' \"$@\"\n",
            hooks_binary().display()
        ),
    );
    let command = |sub: &str| format!("{} {sub}", wrapper.display());
    let hooks = serde_json::json!({"hooks": {
        "PostToolUse": [{"hooks": [{"type": "command", "command": command("post-tool-use")}]}],
        "UserPromptSubmit": [{"hooks": [{"type": "command", "command": command("user-prompt")}]}],
    }});
    fs::write(
        f.codex_home.join("hooks.json"),
        serde_json::to_string_pretty(&hooks).unwrap(),
    )
    .unwrap();
    start_in(f, "codex", standin, &["exec", "-"], Some(b"codex prompt\n"))
}

#[test]
fn a_step_without_a_hook_is_named_to_the_sender_while_it_runs() {
    // A caller's own `--settings` means amplihack passes no hooks settings
    // file, so the step has no hook that can give it new guidance.
    let f = GuideFixture::new();
    let own = f.root.join("own-settings.json");
    fs::write(&own, "{}").unwrap();
    let mut step = start_in(
        f,
        "claude",
        &standin_claude(),
        &["--settings", own.to_str().unwrap()],
        None,
    );
    let id = step.step_id();
    assert!(!id.is_empty());
    let unhooked = step.records("guidance_step_unhooked");
    assert_eq!(unhooked.len(), 1, "{unhooked:?}");
    assert_eq!(unhooked[0]["step_id"], id.as_str());

    let o = step.guide();
    let named = stdout(&o).lines().filter(|l| l.contains(&id)).count();
    assert_eq!(named, 1, "{}", show(&o));

    let run = step.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));
    // Once the step has ended it is no longer named.
    let o = step.guide();
    assert!(!stdout(&o).contains(&id), "{}", show(&o));
    assert!(step.records("guidance_injected").is_empty());
}

#[test]
fn a_hooked_step_whose_hook_never_fires_is_named_to_the_sender_while_it_runs() {
    // The launch finds amplihack's hooks settings file and judges the step
    // hooked; the stand-in harness never runs the hook.
    let mut step = start_with(
        GuideFixture::new(),
        "claude",
        &standin_claude(),
        &["--add-dir", "."],
        None,
        &[("HOOKS_NEVER_FIRE", "1")],
    );
    let id = step.step_id();
    assert!(!id.is_empty());
    assert_eq!(step.records("guidance_step_hooked").len(), 1);
    assert!(step.records("guidance_step_unhooked").is_empty());

    let o = step.guide();
    let named = stdout(&o).lines().filter(|l| l.contains(&id)).count();
    assert_eq!(named, 1, "{}", show(&o));

    let run = step.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));
    assert!(step.records("guidance_step_hook_seen").is_empty());
    assert!(step.records("guidance_injected").is_empty());
    // Once the step has ended it is no longer named.
    let o = step.guide();
    assert!(!stdout(&o).contains(&id), "{}", show(&o));
}

#[test]
fn a_hook_that_cannot_run_deliver_records_the_failure_for_the_sender() {
    // An `amplihack` that fails `recipe guidance deliver` with a status
    // other than its own recorded-failure status (as a crash or an older
    // release without the command would), and is the real one otherwise.
    let f = GuideFixture::new();
    let broken = f.root.join("broken-amplihack");
    executable(
        &broken,
        &format!(
            "#!/bin/sh
[ \"$1 $2 $3\" = 'recipe guidance deliver' ] && exit 3
exec '{}' \"$@\"
",
            amplihack_bin().display()
        ),
    );
    let mut step = start_with(
        f,
        "claude",
        &standin_claude(),
        &["--add-dir", "."],
        None,
        &[("AMPLIHACK_AMPLIHACK_BINARY_PATH", broken.to_str().unwrap())],
    );
    let id = step.step_id();
    // The first hook run, before the append, could not deliver.
    let failed = step.records("guidance_dropped");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["reason"], "hook_delivery_failed");

    let o = step.guide();
    let out = stdout(&o);
    assert!(out.contains("hook_delivery_failed"), "{}", show(&o));
    // Nothing shows that the hook reaches `amplihack`, so it is unconfirmed.
    assert_eq!(
        out.lines().filter(|l| l.contains(&id)).count(),
        1,
        "{}",
        show(&o)
    );

    let run = step.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));
    assert!(!step.context(1).contains(MARKER), "{}", step.context(1));
    assert!(step.records("guidance_injected").is_empty());
    assert!(step.records("guidance_step_hook_seen").is_empty());
    assert_eq!(step.records("guidance_dropped").len(), 3);
}

/// [`start`] with a fixture the caller prepared.
fn start_in(
    f: GuideFixture,
    tool: &str,
    standin: &str,
    tail: &[&str],
    stdin: Option<&[u8]>,
) -> Step {
    start_with(f, tool, standin, tail, stdin, &[])
}

/// [`start_in`] with `env` added to the launch's environment.
fn start_with(
    f: GuideFixture,
    tool: &str,
    standin: &str,
    tail: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Step {
    let run = f.make_run("r-3435", "c-3435");
    let guidance = run.join("guidance");
    // Over the fixture's own fake, so Codex's helper is beside it.
    let agent = f.root.join("tools").join(tool);
    executable(&agent, standin);
    let fifo = f.root.join("release.fifo");
    fifo_release::mkfifo(&fifo);

    let mut args = vec![tool];
    if tool == "claude" {
        args.push("--subprocess-safe");
    }
    args.push("--");
    args.extend_from_slice(tail);
    let mut c: Command = f.amplihack(&args);
    c.env("AMPLIHACK_AGENT_BINARY", tool)
        .env("AMPLIHACK_RUN_GUIDANCE_DIRS", &guidance)
        .env("AMPLIHACK_RECIPE_RUN_ID", "c-3435")
        .env("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", hooks_binary())
        .env("RELEASE_FIFO", &fifo)
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &agent)
        .env("CODEX_BINARY_PATH", &agent)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        c.env(name, value);
    }
    match stdin {
        Some(bytes) => {
            let input = f.root.join("stdin");
            fs::write(&input, bytes).unwrap();
            c.stdin(fs::File::open(input).unwrap());
        }
        None => {
            c.stdin(Stdio::null());
        }
    }
    let live = fifo_release::start(c, &fifo, "amplihack with a stand-in agent");
    Step { f, guidance, live }
}
