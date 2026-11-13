//! Issue #2535: operator guidance reaches the next agent step's prompt.
//!
//! - (a) an entry appended while a recipe is running appears in the next
//!   agent step's prompt, and not in the step that ran before it, for Claude
//!   (`-p` text on stdin) and for Codex (`exec … -`, trailer after the
//!   caller's stdin);
//! - (b) a per-PR entry survives a relaunch: two separate `recipe run`
//!   processes with the same `pr_url`, guidance appended between them;
//! - (s2) a listed folder that fails the safety checks is skipped and the
//!   prompt is unchanged;
//! - (u4) with no guidance the prompt reaches the agent byte for byte.
//!
//! The recipe runner is a fake that calls the real launcher, so the path
//! under test is `recipe run` → runner → `amplihack claude|codex` → agent.
//! Entries are appended by the runner itself between its two steps, which
//! is "while the recipe is running" without any timer or wait. They stand in
//! for an operator outside the run, so they call `recipe guide` with
//! `AMPLIHACK_RECIPE_RUN_ID` unset (inside a run it is refused without
//! `--from-step`).

#![cfg(unix)]

use crate::support::recipe_guide::{
    BEGIN, END, GuideFixture, PR_KEY, PR_URL, parse_appended, private_dir, show, stderr,
};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const MARKER: &str = "MARKER-2535-run-the-crate-tests-only";

/// Two Claude steps; between them the runner appends `$GUIDE_TEXT` to its own
/// run, addressed by `$GUIDE_BY` (`run_id` or `corr_id`).
const TWO_CLAUDE_STEPS: &str = r#"
STEP=a.1 "$AMP_BIN" claude --subprocess-safe -- -p "step one prompt" \
  > "$RECORD_DIR/out.a.1" 2> "$RECORD_DIR/err.a.1" || exit 11
case "$GUIDE_BY" in
  corr_id) target="$AMPLIHACK_RECIPE_RUN_ID" ;;
  *) target="$AMPLIHACK_RUN_ID" ;;
esac
( unset AMPLIHACK_RECIPE_RUN_ID; "$AMP_BIN" recipe guide "$target" "$GUIDE_TEXT" ) \
  > "$RECORD_DIR/guide.out" 2> "$RECORD_DIR/guide.err" || exit 12
STEP=a.2 "$AMP_BIN" claude --subprocess-safe -- -p "step two prompt" \
  > "$RECORD_DIR/out.a.2" 2> "$RECORD_DIR/err.a.2" || exit 13
"#;

/// The same with Codex `exec --output-last-message F -`, prompt on stdin.
const TWO_CODEX_STEPS: &str = r#"
printf 'codex step one prompt\n' | STEP=c.1 "$AMP_BIN" codex -- exec \
  --output-last-message "$RECORD_DIR/last.c.1" - \
  > "$RECORD_DIR/out.c.1" 2> "$RECORD_DIR/err.c.1" || exit 21
target="$AMPLIHACK_RECIPE_RUN_ID"
( unset AMPLIHACK_RECIPE_RUN_ID; "$AMP_BIN" recipe guide "$target" "$GUIDE_TEXT" ) \
  > "$RECORD_DIR/guide.out" 2> "$RECORD_DIR/guide.err" || exit 22
printf 'codex step two prompt\n' | STEP=c.2 "$AMP_BIN" codex -- exec \
  --output-last-message "$RECORD_DIR/last.c.2" - \
  > "$RECORD_DIR/out.c.2" 2> "$RECORD_DIR/err.c.2" || exit 23
"#;

/// One Claude step tagged by `$RUN_TAG`.
const ONE_CLAUDE_STEP: &str = r#"
STEP="$RUN_TAG.1" "$AMP_BIN" claude --subprocess-safe -- -p "drive step prompt" \
  > "$RECORD_DIR/out.$RUN_TAG.1" 2> "$RECORD_DIR/err.$RUN_TAG.1" || exit 31
"#;

fn run_folder(f: &GuideFixture, tag: &str) -> PathBuf {
    f.runs().join(f.record(&format!("run_id.{tag}")).trim())
}

fn assert_block_once(prompt: &str, text: &str, source: &str) {
    assert_eq!(prompt.matches(BEGIN).count(), 1, "one block:\n{prompt}");
    assert_eq!(prompt.matches(END).count(), 1, "one block:\n{prompt}");
    let begin = prompt.find(BEGIN).unwrap();
    let end = prompt.find(END).unwrap();
    let block = &prompt[begin..end];
    assert!(
        block.contains("Operator instructions for this run"),
        "{block}"
    );
    assert!(block.contains(text), "{block}");
    assert!(
        block
            .lines()
            .any(|l| l.starts_with("[1] ") && l.ends_with(&format!("({source})"))),
        "entry header labelled ({source}):\n{block}"
    );
    assert!(
        prompt[end..].trim_end() == END,
        "nothing after the block:\n{prompt}"
    );
}

// ---------------------------------------------------------------------------
// (a) Claude
// ---------------------------------------------------------------------------

#[test]
fn a_entry_appended_mid_run_reaches_the_next_claude_step_only() {
    let f = GuideFixture::new();
    let o = f.recipe_run(
        TWO_CLAUDE_STEPS,
        &[],
        &[("GUIDE_TEXT", MARKER), ("GUIDE_BY", "run_id")],
    );
    assert!(
        o.status.success(),
        "{}\nguide.err: {:?}",
        show(&o),
        f.record_opt("guide.err")
    );

    // The runner saw the list with its own run folder's guidance in it.
    let run = run_folder(&f, "run");
    let guidance = run.join("guidance");
    let dirs: Vec<PathBuf> = std::env::split_paths(&OsString::from(f.record("dirs.run"))).collect();
    assert!(
        dirs.contains(&guidance),
        "{dirs:?} lacks {}",
        guidance.display()
    );

    // Step one ran before the entry existed: its prompt is untouched.
    assert_eq!(f.record("prompt.a.1"), "step one prompt");
    assert!(!f.record("err.a.1").contains("operator guidance"));

    // Step two receives the block after exactly one blank line.
    let p2 = f.record("prompt.a.2");
    assert!(p2.starts_with("step two prompt\n\n"), "{p2:?}");
    assert!(
        p2["step two prompt\n\n".len()..].starts_with(BEGIN),
        "{p2:?}"
    );
    assert_block_once(&p2, MARKER, "run");

    // One stderr line with the counts, never the text.
    let err2 = f.record("err.a.2");
    let note: Vec<&str> = err2
        .lines()
        .filter(|l| l.starts_with("amplihack: operator guidance added to this step's prompt"))
        .collect();
    assert_eq!(
        note,
        [format!(
            "amplihack: operator guidance added to this step's prompt (1 entry, {} bytes)",
            MARKER.len()
        )
        .as_str()],
        "{err2}"
    );
    assert!(!err2.contains(MARKER));
    assert!(
        !stderr(&o).contains(MARKER),
        "never in amplihack's messages"
    );

    // Evidence: one append and one injection, no text.
    let log = f.log(&guidance);
    let events: Vec<&str> = log.iter().map(|l| l["event"].as_str().unwrap()).collect();
    assert_eq!(events, ["guidance_appended", "guidance_injected"]);
    assert_eq!(log[1]["entries"], 1);
    assert_eq!(log[1]["bytes"], MARKER.len());
    assert_eq!(
        log[1]["step_run_id"].as_str().unwrap(),
        f.record("corr_id.run").trim()
    );
    let raw = fs::read_to_string(run.join("guidance.log")).unwrap();
    assert!(!raw.contains(MARKER));
    assert!(
        !fs::read_to_string(run.join("run.json"))
            .unwrap()
            .contains(MARKER)
    );
}

#[test]
fn a_entry_appended_by_correlation_id_reaches_the_next_step() {
    let f = GuideFixture::new();
    let o = f.recipe_run(
        TWO_CLAUDE_STEPS,
        &[],
        &[("GUIDE_TEXT", MARKER), ("GUIDE_BY", "corr_id")],
    );
    assert!(
        o.status.success(),
        "{}\nguide.err: {:?}",
        show(&o),
        f.record_opt("guide.err")
    );
    assert_eq!(f.record("prompt.a.1"), "step one prompt");
    assert_block_once(&f.record("prompt.a.2"), MARKER, "run");
    // The entry is in the run folder, whichever ID named it.
    let run = run_folder(&f, "run");
    assert_eq!(f.entries(&run.join("guidance")).len(), 1);
}

// ---------------------------------------------------------------------------
// (a) Codex: trailer after the caller's stdin
// ---------------------------------------------------------------------------

#[test]
fn a_entry_appended_mid_run_reaches_the_next_codex_exec_step_as_a_trailer() {
    let f = GuideFixture::new();
    let o = f.recipe_run(TWO_CODEX_STEPS, &[], &[("GUIDE_TEXT", MARKER)]);
    assert!(
        o.status.success(),
        "{}\nguide.err: {:?}",
        show(&o),
        f.record_opt("guide.err")
    );

    assert_eq!(f.record("prompt.c.1"), "codex step one prompt\n");
    let p2 = f.record("prompt.c.2");
    assert!(p2.starts_with("codex step two prompt\n"), "{p2:?}");
    assert_block_once(&p2, MARKER, "run");
    // The argv still ends in "-": nothing about guidance reaches argv.
    let argv = f.record("codexargv.c.2");
    assert_eq!(argv.lines().last(), Some("-"));
    assert!(!argv.contains(MARKER) && !argv.contains("OPERATOR"));
}

// ---------------------------------------------------------------------------
// (b) per-PR guidance survives a relaunch of the drive
// ---------------------------------------------------------------------------

#[test]
fn b_per_pr_entry_survives_a_relaunch_and_the_run_folders_differ() {
    let f = GuideFixture::new();
    let ctx = format!("pr_url={PR_URL}");

    // First drive: no guidance yet.
    let first = f.recipe_run(ONE_CLAUDE_STEP, &["-c", &ctx], &[("RUN_TAG", "first")]);
    assert!(first.status.success(), "{}", show(&first));
    assert_eq!(f.record("prompt.first.1"), "drive step prompt");

    // The operator adds guidance by PR URL between the two drives.
    let g = f.guide(&[PR_URL, MARKER], None);
    assert!(g.status.success(), "{}", show(&g));
    let (entry, _, source) = parse_appended(&g);
    assert_eq!(source, "pr");
    let pr_guidance = f.state.join("auto-drive").join(PR_KEY).join("guidance");
    assert_eq!(entry.parent(), Some(pr_guidance.as_path()));

    // Relaunch: a new process and a new run folder, same PR.
    let second = f.recipe_run(ONE_CLAUDE_STEP, &["-c", &ctx], &[("RUN_TAG", "second")]);
    assert!(second.status.success(), "{}", show(&second));
    assert_ne!(
        f.record("run_id.first"),
        f.record("run_id.second"),
        "the relaunch has its own run folder"
    );
    let dirs: Vec<PathBuf> =
        std::env::split_paths(&OsString::from(f.record("dirs.second"))).collect();
    assert!(dirs.contains(&pr_guidance), "{dirs:?}");
    assert!(
        !run_folder(&f, "second").join("guidance").exists()
            || f.entries(&run_folder(&f, "second").join("guidance"))
                .is_empty(),
        "the entry came from the per-PR folder, not the run folder"
    );

    let p = f.record("prompt.second.1");
    assert!(p.starts_with("drive step prompt\n\n"), "{p:?}");
    assert_block_once(&p, MARKER, "pr");

    let events: Vec<String> = f
        .log(&pr_guidance)
        .iter()
        .map(|l| l["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(events, ["guidance_appended", "guidance_injected"]);
}

#[test]
fn b_pr_number_with_an_ssh_origin_reads_the_same_per_pr_folder() {
    let f = GuideFixture::new();
    let repo = f.root.join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let s = amplihack_git::command()
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &f.home)
            .status()
            .unwrap();
        assert!(s.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "git@github.com:o/r.git"]);

    let g = f.guide(&[PR_URL, MARKER], None);
    assert!(g.status.success(), "{}", show(&g));

    let repo_ctx = format!("repo_path={}", repo.display());
    let o = f.recipe_run(
        ONE_CLAUDE_STEP,
        &["-c", "pr_number=7", "-c", &repo_ctx],
        &[("RUN_TAG", "ssh")],
    );
    assert!(o.status.success(), "{}", show(&o));
    assert_block_once(&f.record("prompt.ssh.1"), MARKER, "pr");
}

// ---------------------------------------------------------------------------
// (u4, s2) the launcher on its own
// ---------------------------------------------------------------------------

#[test]
fn u4_without_the_variable_or_entries_the_prompt_is_unchanged() {
    let f = GuideFixture::new();
    let o = f.launch_claude("plain", "exact prompt\n\n  ", None);
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(f.record("prompt.plain"), "exact prompt\n\n  ");
    assert!(!stderr(&o).contains("operator guidance"));

    // An empty, private guidance folder on the list changes nothing either.
    let run = f.make_run("r-empty", "c-empty");
    let guidance = run.join("guidance");
    private_dir(&guidance);
    let o = f.launch_claude("empty", "exact prompt", Some(guidance.as_os_str()));
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(f.record("prompt.empty"), "exact prompt");
    assert!(f.log(&guidance).is_empty(), "no injection is logged");
}

#[test]
fn s2_unsafe_or_misnamed_listed_folders_are_skipped_with_a_warning() {
    let f = GuideFixture::new();
    // A real entry in a folder that is then made group-writable.
    let run = f.make_run("r-unsafe", "c-unsafe");
    let g = f.guide(&["r-unsafe", MARKER], None);
    assert!(g.status.success(), "{}", show(&g));
    let guidance = run.join("guidance");
    fs::set_permissions(&guidance, fs::Permissions::from_mode(0o770)).unwrap();

    // A private folder holding a real entry but not named `guidance`.
    let other = f.make_run("r-other", "c-other");
    assert!(f.guide(&["r-other", MARKER], None).status.success());
    let renamed = other.join("notes");
    fs::rename(other.join("guidance"), &renamed).unwrap();

    let list = std::env::join_paths([
        guidance.clone(),
        renamed.clone(),
        PathBuf::from("relative/guidance"),
    ])
    .unwrap();
    let o = f.launch_claude("unsafe", "the prompt", Some(&list));
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(f.record("prompt.unsafe"), "the prompt");
    let err = stderr(&o);
    assert!(
        err.contains(&guidance.display().to_string()),
        "warning names the folder:\n{err}"
    );
    assert!(err.contains(&renamed.display().to_string()), "{err}");
    assert!(!err.contains(MARKER));
}

#[test]
fn codex_forms_other_than_exec_dash_get_no_guidance_and_one_note() {
    let f = GuideFixture::new();
    let run = f.make_run("r-codex", "c-codex");
    assert!(f.guide(&["r-codex", MARKER], None).status.success());
    let guidance = run.join("guidance");
    let last = f.root.join("last-message");
    let last = last.to_str().unwrap();

    // Positional prompt: rewritten to stdin by the launcher, but not the
    // `exec … -` form the caller chose, so no guidance (C4).
    let o = f.launch_codex(
        "positional",
        &["exec", "--output-last-message", last, "positional prompt"],
        b"",
        Some(guidance.as_os_str()),
    );
    assert!(o.status.success(), "{}", show(&o));
    assert!(!f.record("prompt.positional").contains(BEGIN));
    let notes = stderr(&o)
        .lines()
        .filter(|l| l.contains("guidance"))
        .count();
    assert_eq!(notes, 1, "one note on stderr:\n{}", stderr(&o));

    // The `exec … -` form receives it after the caller's stdin.
    let o = f.launch_codex(
        "dash",
        &["exec", "--output-last-message", last, "-"],
        b"piped prompt\n",
        Some(guidance.as_os_str()),
    );
    assert!(o.status.success(), "{}", show(&o));
    let p = f.record("prompt.dash");
    assert!(p.starts_with("piped prompt\n"), "{p:?}");
    assert_block_once(&p, MARKER, "run");
}
