//! Issue #3364: a run that waits for its owner leaves `waiting.json` in its
//! run folder, and `amplihack recipe waiting` lists it; a run that ends with
//! open decisions leaves `open-decisions.json`, listed until guidance answers
//! it or its pull request is merged or closed.
//!
//! The turn tool is the bundle's `autodrive_goal_turn.sh wait-owner`, run as
//! a step of a real `amplihack recipe run` (the #2535 fixture's runner
//! stand-in), so the run is live with its own run folder, process record and
//! guidance folder. `AMPLIHACK_BIN` points the turn tool at a wrapper that
//! blocks on a FIFO before it starts the real `amplihack recipe guidance
//! wait`: the FIFO open is the rendezvous, so the waiting record has been
//! written when the test reads it. No sleep, poll or timer.
//!
//! The tests compare JSON fields, paths, counts and exit codes, never wording.
//! The open-decisions record is written by an agent step whose output cannot
//! be run deterministically, so its test writes a record of the shape that
//! step is asked for and checks what the listing does with it.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::fifo_release;
use crate::support::recipe_guide::{GuideFixture, PR_URL, private_dir, private_file, show, stdout};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const SESSION: &str = "build-3364-waiting";
const OWNER: &str = "the operator";
const QUESTION: &str = "Merge now, or wait for the second review?";

/// Blocks `recipe guidance wait` on a FIFO, then runs the real binary.
/// Instance `a` waits for instance `b`; `b` opens `a`'s FIFO (which returns
/// once `a` is blocked on it), then blocks on the test's FIFO, so when the
/// test's open returns both turn tools have written their record.
const WAIT_WRAPPER: &str = r#"#!/bin/sh
case "$1 $2 $3" in
  "recipe guidance wait")
    case "${INSTANCE:-}" in
      a) read _ < "$FIFO_A" ;;
      b) exec 3> "$FIFO_A"; read _ < "$RELEASE_FIFO"; exec 3>&- ;;
      *) read _ < "$RELEASE_FIFO" ;;
    esac ;;
esac
exec "$AMP_BIN" "$@"
"#;

/// The turn tool with the turn's environment. `$1` names the instance.
const TURN: &str = r#"
turn() {
  INSTANCE="$1" AUTODRIVE_STATE_DIR="$ASTATE" AMPLIHACK_BIN="$WAIT_BIN" AMPLIHACK_RECIPE_LOG="$RUN_LOG" \
    bash "$TURN_TOOL" wait-owner "$ASTATE/turns/001" > "$RECORD_DIR/turn.out.$1" 2> "$RECORD_DIR/turn.err.$1"
  echo $? > "$RECORD_DIR/turn.rc.$1"
}
"#;

fn turn_tool() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/autodrive_goal_turn.sh")
}

struct Drive {
    f: GuideFixture,
    astate: PathBuf,
    run_log: PathBuf,
}

/// A fixture with one turn folder whose decision is `wait-owner`.
fn drive() -> Drive {
    let f = GuideFixture::new();
    let astate = f.root.join("astate");
    let turn = astate.join("turns").join("001");
    for d in [&astate, &astate.join("turns"), &turn] {
        private_dir(d);
    }
    private_file(
        &turn.join("reading.json"),
        json!({"schema_version": 1, "goal": {"summary": "s"}, "guidance_newest": ""}).to_string(),
    );
    private_file(
        &turn.join("decision.json"),
        json!({"action": "wait-owner", "args": {"owner": OWNER, "question": QUESTION}, "why": "w"})
            .to_string(),
    );
    let run_log = f.root.join("run.log");
    private_file(&run_log, "");
    crate::support::exec_stub::write_executable(
        &f.tools.join("wait-amplihack"),
        WAIT_WRAPPER,
        0o755,
    );
    Drive { f, astate, run_log }
}

impl Drive {
    /// Starts `recipe run` with `body` and returns once it is blocked on
    /// `release`, with the session log that names the run written.
    fn start(&self, body: &str, release: &Path, fifo_a: &Path) -> (fifo_release::Live, PathBuf) {
        let tool = turn_tool();
        let wait_bin = self.f.tools.join("wait-amplihack");
        let command = self.f.recipe_run_command(
            &format!("{TURN}{body}"),
            &[],
            &[
                ("ASTATE", self.astate.to_str().unwrap()),
                ("RUN_LOG", self.run_log.to_str().unwrap()),
                ("TURN_TOOL", tool.to_str().unwrap()),
                ("WAIT_BIN", wait_bin.to_str().unwrap()),
                ("RELEASE_FIFO", release.to_str().unwrap()),
                ("FIFO_A", fifo_a.to_str().unwrap()),
            ],
        );
        let live = fifo_release::start(command, release, "amplihack recipe run");
        let run_dir = self.f.runs().join(self.f.record("run_id.run").trim());
        let pointer = GuideFixture::pointer(self.f.record("corr_id.run").trim(), Some(&run_dir));
        self.f.session_log_from_pointer(SESSION, &pointer);
        (live, run_dir)
    }

    fn fifo(&self, name: &str) -> PathBuf {
        let p = self.f.root.join(name);
        fifo_release::mkfifo(&p);
        p
    }
}

/// `amplihack recipe waiting --json`, parsed.
fn listing(f: &GuideFixture) -> Vec<Value> {
    let o = f.run_with_stdin(f.amplihack(&["recipe", "waiting", "--json"]), None);
    assert!(o.status.success(), "{}", show(&o));
    serde_json::from_str::<Vec<Value>>(stdout(&o).trim())
        .unwrap_or_else(|e| panic!("{e}: {}", show(&o)))
}

fn for_dir<'a>(items: &'a [Value], dir: &Path) -> Vec<&'a Value> {
    items
        .iter()
        .filter(|i| i["run_dir"] == dir.display().to_string())
        .collect()
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap()
}

#[test]
fn a_wait_owner_turn_writes_the_record_before_it_blocks_and_the_listing_drops_it_once_guidance_arrives()
 {
    let d = drive();
    let release = d.fifo("release.fifo");
    let fifo_a = d.fifo("a.fifo");
    let body = r#"turn one
"$AMP_BIN" recipe waiting --json > "$RECORD_DIR/after.json" 2> "$RECORD_DIR/after.err"
"#;
    let (mut live, run_dir) = d.start(body, &release, &fifo_a);

    let record = read_json(&run_dir.join("waiting.json"));
    assert_eq!(record["kind"], "waiting");
    assert_eq!(record["owner"], OWNER);
    assert_eq!(record["question"], QUESTION);
    assert_eq!(record["turn"], "001");
    assert_eq!(record["run_id"], d.f.record("run_id.run").trim());
    assert!(record["started_at"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(
        fs::read_to_string(&d.run_log).unwrap().lines().count(),
        1,
        "the run log gains one line when the wait begins"
    );

    let items = listing(&d.f);
    let mine = for_dir(&items, &run_dir);
    assert_eq!(mine.len(), 1, "{items:?}");
    assert_eq!(mine[0]["kind"], "waiting");
    assert_eq!(mine[0]["owner"], OWNER);
    assert_eq!(mine[0]["question"], QUESTION);
    assert!(
        mine[0]["age_seconds"].as_i64().is_some_and(|a| a >= 0),
        "{:?}",
        mine[0]
    );

    let text =
        d.f.run_with_stdin(d.f.amplihack(&["recipe", "waiting"]), None);
    assert!(text.status.success(), "{}", show(&text));
    let lines: Vec<String> = stdout(&text).lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let fields: Vec<&str> = lines[0].split('\t').collect();
    assert_eq!(fields.len(), 5, "{lines:?}");
    assert_eq!(fields[0], "waiting");
    assert_eq!(fields[2], OWNER);
    assert_eq!(fields[4], d.f.record("run_id.run").trim());

    let o = d.f.guide_text(&[SESSION], "Merge it.");
    assert!(o.status.success(), "{}", show(&o));
    let out = live.finish();
    assert!(out.status.success(), "{}", show(&out));
    assert_eq!(
        d.f.record("turn.rc.one").trim(),
        "0",
        "{}",
        d.f.record("turn.err.one")
    );
    assert_eq!(
        read_json(&d.astate.join("turns/001/result.json"))["outcome"],
        "guidance-arrived"
    );
    assert!(
        !run_dir.join("waiting.json").exists(),
        "the record is removed when the wait returns"
    );
    let after: Vec<Value> = serde_json::from_str(d.f.record("after.json").trim()).unwrap();
    assert!(
        for_dir(&after, &run_dir).is_empty(),
        "listed while still live after guidance arrived: {after:?}"
    );
    assert_eq!(fs::read_to_string(&d.run_log).unwrap().lines().count(), 1);
}

#[test]
fn a_run_waiting_for_its_owner_is_listed_once_though_two_turn_tools_wait() {
    let d = drive();
    let release = d.fifo("release.fifo");
    let fifo_a = d.fifo("a.fifo");
    let body = "turn a & pa=$!\nturn b & pb=$!\nwait $pa\nwait $pb\n";
    let (mut live, run_dir) = d.start(body, &release, &fifo_a);

    let items = listing(&d.f);
    let mine = for_dir(&items, &run_dir);
    assert_eq!(mine.len(), 1, "{items:?}");
    assert_eq!(mine[0]["kind"], "waiting");

    let o = d.f.guide_text(&[SESSION], "Merge it.");
    assert!(o.status.success(), "{}", show(&o));
    let out = live.finish();
    assert!(out.status.success(), "{}", show(&out));
    for i in ["a", "b"] {
        assert_eq!(
            d.f.record(&format!("turn.rc.{i}")).trim(),
            "0",
            "{}",
            d.f.record(&format!("turn.err.{i}"))
        );
    }
    assert!(!run_dir.join("waiting.json").exists());
}

#[test]
fn the_fallback_hand_off_leaves_no_waiting_record() {
    // No guidance folder is listed and no target is named, so the real
    // `recipe guidance wait` cannot arm a wake and exits 3.
    let d = drive();
    let run_dir = d.f.make_run("3364-fallback-run", "corr-3364-fallback");
    let goal_tool = d.f.tools.join("goal-tool.sh");
    crate::support::exec_stub::write_executable(
        &goal_tool,
        "#!/bin/bash\n[ \"$1\" = publish ] && cp -- \"$2\" \"$FIXTURE_DIR/published.md\"\n",
        0o755,
    );
    let tool = turn_tool();
    let td = d.astate.join("turns/001");
    let mut c = d.f.amplihack_at(
        Path::new("/bin/bash"),
        &[tool.to_str().unwrap(), "wait-owner", td.to_str().unwrap()],
    );
    c.env("AUTODRIVE_STATE_DIR", &d.astate)
        .env("AMPLIHACK_BIN", amplihack_bin())
        .env("AMPLIHACK_RUN_DIR", &run_dir)
        .env("AMPLIHACK_RUN_ID", "3364-fallback-run")
        .env("AMPLIHACK_RECIPE_LOG", &d.run_log)
        .env("AUTODRIVE_GOAL_TOOL", &goal_tool)
        .env_remove("AMPLIHACK_RUN_GUIDANCE_DIRS");
    let o = d.f.run_with_stdin(c, None);
    assert!(o.status.success(), "{}", show(&o));
    let result = read_json(&td.join("result.json"));
    assert_eq!(result["outcome"], "ended", "{result}");
    assert_eq!(result["end"], "handoff", "{result}");
    assert!(
        d.f.root.join("published.md").is_file(),
        "the hand-off was published"
    );
    assert!(!run_dir.join("waiting.json").exists());
    assert_eq!(fs::read_to_string(&d.run_log).unwrap().lines().count(), 1);
}

#[test]
fn a_waiting_record_left_in_a_run_that_is_not_live_is_not_listed() {
    let f = GuideFixture::new();
    let ended = f.make_run("3364-ended-run", "corr-3364-ended");
    let mut report = read_json(&ended.join("run.json"));
    report["ended_at"] = "2026-11-26T10:00:00Z".into();
    report["outcome"] = "success".into();
    private_file(&ended.join("run.json"), report.to_string());
    // Not ended, but no runner is alive for it.
    let gone = f.make_run("3364-gone-run", "corr-3364-gone");
    let mut pointers = Vec::new();
    for (dir, corr) in [(&ended, "corr-3364-ended"), (&gone, "corr-3364-gone")] {
        private_file(
            &dir.join("waiting.json"),
            json!({"kind": "waiting", "run_id": corr, "target": "", "owner": OWNER,
                   "question": QUESTION, "turn": "001", "started_at": "2026-11-26T09:00:00Z"})
            .to_string(),
        );
        pointers.push(GuideFixture::pointer(corr, Some(dir)));
    }
    f.session_log_from_pointers(SESSION, &pointers);
    assert!(
        listing(&f).iter().all(|i| i["kind"] != "waiting"),
        "{:?}",
        listing(&f)
    );
    let text = f.run_with_stdin(f.amplihack(&["recipe", "waiting"]), None);
    assert!(text.status.success(), "{}", show(&text));
    assert!(stdout(&text).trim().is_empty(), "{}", show(&text));
}

/// A stand-in `gh` that reports the state in `$FIXTURE_DIR/pr_state`, or
/// fails when it holds `fail`.
const FAKE_GH_STATE: &str = r#"#!/bin/sh
s="$(cat "$FIXTURE_DIR/pr_state")"
[ "$s" = fail ] && exit 1
printf '{"state":"%s"}\n' "$s"
"#;

fn open_decisions(f: &GuideFixture, folder: &str, target: &str) -> PathBuf {
    let dir = f.make_run(folder, &format!("corr-{folder}"));
    private_file(
        &dir.join("open-decisions.json"),
        json!({"kind": "open-decisions", "run_id": folder, "target": target,
               "questions": [QUESTION, "Keep the old flag?"],
               "where": [format!("{target}#issuecomment-1")],
               "created_at": "2026-01-01T00:00:00Z"})
        .to_string(),
    );
    dir
}

#[test]
fn an_open_decisions_record_is_listed_until_guidance_answers_it_or_the_pull_request_closes() {
    let f = GuideFixture::new();
    crate::support::exec_stub::write_executable(&f.tools.join("gh"), FAKE_GH_STATE, 0o755);
    let state = |s: &str| fs::write(f.root.join("pr_state"), s).unwrap();
    let pr8 = PR_URL.replace("/7", "/8");
    let answered = open_decisions(&f, "3364-open-a", PR_URL);
    let other = open_decisions(&f, "3364-open-b", &pr8);

    state("OPEN");
    let items = listing(&f);
    for dir in [&answered, &other] {
        let mine = for_dir(&items, dir);
        assert_eq!(mine.len(), 1, "{items:?}");
        assert_eq!(mine[0]["kind"], "open-decisions");
        assert_eq!(mine[0]["questions"].as_array().map(Vec::len), Some(2));
        assert_eq!(mine[0]["where"].as_array().map(Vec::len), Some(1));
        assert_eq!(mine[0]["pr_state"], "OPEN");
        assert!(mine[0]["age_seconds"].as_i64().is_some_and(|a| a > 0));
    }

    state("fail");
    let items = listing(&f);
    assert_eq!(for_dir(&items, &answered).len(), 1, "{items:?}");
    assert_eq!(for_dir(&items, &answered)[0]["pr_state"], "unknown");

    for closed in ["CLOSED", "MERGED"] {
        state(closed);
        let items = listing(&f);
        assert!(
            for_dir(&items, &answered).is_empty() && for_dir(&items, &other).is_empty(),
            "{items:?}"
        );
    }

    state("OPEN");
    let o = f.guide_text(&["3364-open-a"], "Keep the old flag.");
    assert!(o.status.success(), "{}", show(&o));
    let items = listing(&f);
    assert!(
        for_dir(&items, &answered).is_empty(),
        "answered by guidance: {items:?}"
    );
    assert_eq!(for_dir(&items, &other).len(), 1, "{items:?}");
}
