//! Issue #2528: a recipe run uses one bundle for its recipes and its tools.
//!
//! Behaviour tests against the real `amplihack` binary and, where a step
//! must run, the real `recipe-runner-rs`:
//!
//! - T1: a checkout whose bundle is newer than the installed one (its recipe
//!   passes `--invocation`, which only its own tool accepts). The default run
//!   uses the installed recipe and tool; `--bundle checkout` uses the
//!   checkout's recipe and tool. Neither prints `unknown argument`. The log
//!   records name one bundle. `-c bundle=` is equivalent to the flag; a
//!   conflict between the two is refused.
//! - T2: an untrusted checkout bundle is refused before any step runs.
//! - T3: replacing a flat installed bundle during a run does not change the
//!   tools, sub-recipes or nested runs of that run.
//! - T4: the same with a release symlink that is re-pointed during the run.
//! - T6: the state home steps see is the real state root, not the snapshot.
//!
//! Every directory is under a private temporary root: `HOME`,
//! `AMPLIHACK_HOME` and `AMPLIHACK_WORK_ROOT` never name the real ones. Every
//! child is joined with `wait()` (through `output()`); there are no sleeps,
//! polls or timeouts. Linux only, like the other real-runner tests.
//!
//! - B1: a recipe given as a path runs from the run's bundle when it lies in
//!   that bundle, and is refused (E14) when it lies in another bundle.
//! - T6b: steps of a pinned run find a git identity kept under a
//!   non-default `AMPLIHACK_HOME`.
//!
//! The real runner is the receipt-checked pinned build resolved by
//! `tests/support/pinned_runner.rs` (#2658), never a `PATH` lookup. Tests
//! that need a step to run fail without one, as the #1741 tests do; they
//! never skip.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const POINTER: &str = "amplihack.recipe.log_pointer ";

/// The pinned runner (#2658): a receipt-checked override or a build cached in
/// this test target's own temporary directory, never the host's installed
/// runner and never a `PATH` lookup.
///
/// These tests drive real recipe runs, so this binary executes the fixtures'
/// steps. Resolved through the shared helper, which puts every source through
/// the install-receipt check and has no fallback: a source that fails the
/// check stops the test instead of quietly supplying a substitute.
fn real_runner() -> PathBuf {
    crate::pinned_runner::pinned_runner(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// The real runner at the pin; the test name stays for the call sites.
///
/// A test that needs a step to run fails without a runner: the shared helper
/// panics, naming the source that failed the receipt check. The parameter is
/// kept so that the lookup can move again without touching the thirteen
/// places that call this, each of which still reads as the test it belongs
/// to.
pub(crate) fn runner(_test: &str) -> PathBuf {
    real_runner()
}

pub(crate) fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("UTF-8 temp path");
    assert!(!text.contains('\''), "temp path holds a quote: {text}");
    format!("'{text}'")
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Writes `rel` under `bundle` with an explicit mode, creating 0755 parents,
/// so the fixture is trusted whatever the test process umask.
pub(crate) fn put(bundle: &Path, rel: &str, body: &str, mode: u32) {
    let path = bundle.join(rel);
    let mut dir = path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    while dir.starts_with(bundle) {
        set_mode(&dir, 0o755);
        if !dir.pop() {
            break;
        }
    }
    fs::write(&path, body).unwrap();
    set_mode(&path, mode);
}

/// A one-or-more-step recipe; each `(id, body)` becomes a bash step.
pub(crate) fn recipe(name: &str, label: &str, steps: &[(&str, String)]) -> String {
    let mut text = format!(
        "name: {name}\ndescription: issue 2528 {label}\nversion: 1.0.0\ncontext: {{}}\nsteps:\n"
    );
    for (id, body) in steps {
        let block: String = body.lines().map(|l| format!("      {l}\n")).collect();
        text.push_str(&format!(
            "  - id: {id}\n    type: bash\n    command: |\n{block}"
        ));
    }
    text
}

/// A private world: user HOME, installed home, work root and records.
pub(crate) struct World {
    _tmp: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) installed: PathBuf,
    pub(crate) work: PathBuf,
    pub(crate) records: PathBuf,
}

impl World {
    pub(crate) fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let world = Self {
            home: root.join("home"),
            installed: root.join("installed"),
            work: root.join("work"),
            records: root.join("records"),
            root,
            _tmp: tmp,
        };
        for dir in [&world.home, &world.installed, &world.work, &world.records] {
            fs::create_dir_all(dir).unwrap();
            set_mode(dir, 0o755);
        }
        // The recovery store refuses a work root that is not private.
        set_mode(&world.work, 0o700);
        world
    }

    pub(crate) fn out(&self) -> PathBuf {
        self.records.join("out.txt")
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        fs::read_to_string(self.out())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// `amplihack recipe run <args>` with a private environment, joined.
    pub(crate) fn run(&self, runner: Option<&Path>, cwd: &Path, args: &[&str]) -> Output {
        self.command(runner, cwd, args)
            .output()
            .expect("run amplihack recipe run")
    }

    pub(crate) fn command(&self, runner: Option<&Path>, cwd: &Path, args: &[&str]) -> Command {
        let mut path_env = String::from("/usr/local/bin:/usr/bin:/bin");
        if let Some(runner) = runner {
            path_env = format!("{}:{path_env}", runner.parent().unwrap().display());
        }
        let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
        cmd.args(["recipe", "run"])
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env("PATH", &path_env)
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.installed)
            .env("AMPLIHACK_WORK_ROOT", &self.work)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .stdin(Stdio::null());
        match runner {
            Some(runner) => cmd.env("RECIPE_RUNNER_RS_PATH", runner),
            None => cmd.env("RECIPE_RUNNER_RS_PATH", self.root.join("no-runner")),
        };
        cmd
    }
}

/// Initialises a git checkout at `dir` with one empty commit. Issue #2096
/// (PR #2290): a run clones the caller's checkout, and it refuses one whose
/// HEAD is unborn. The bundle files written after this stay uncommitted; the
/// run reads them from its pinned snapshot, never from the clone.
pub(crate) fn git_init(dir: &Path) {
    crate::support::git_fixture::init_with_commit(dir);
}

/// The log-pointer records on stderr, as JSON.
pub(crate) fn records(stderr: &str) -> Vec<serde_json::Value> {
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix(POINTER))
        .map(|json| serde_json::from_str(json).expect("log pointer is JSON"))
        .collect()
}

pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Every early/final record names one bundle for recipes and tools, from
/// `source`, and returns the early record's snapshot path.
pub(crate) fn assert_one_bundle(stderr: &str, source: &Path, choice: &str) -> Option<PathBuf> {
    let records = records(stderr);
    assert!(!records.is_empty(), "no log records:\n{stderr}");
    let mut snapshot = None;
    for record in &records {
        if !matches!(record["event"].as_str(), Some("early" | "final")) {
            continue;
        }
        let recipe = &record["recipe_bundle"];
        assert!(recipe.is_object(), "recipe_bundle missing: {record}");
        assert_eq!(recipe, &record["tool_bundle"], "one bundle: {record}");
        assert_eq!(recipe["source"], source.to_str().unwrap(), "{record}");
        assert_eq!(recipe["choice"], choice, "{record}");
        if record["event"] == "early" {
            snapshot = recipe["snapshot"].as_str().map(PathBuf::from);
        }
    }
    snapshot
}

// ---------------------------------------------------------------------- T1

/// Installed bundle A (old) and checkout bundle B (new) with the same
/// `split-probe` recipe name. B's recipe passes `--invocation`; only B's tool
/// accepts it. Both write `recipe=<X>` and `tool=<X>` to the out file.
fn split_world() -> (World, PathBuf) {
    let world = World::new();
    let out = quoted(&world.out());
    let a = world.installed.join("amplifier-bundle");
    put(
        &a,
        "tools/split_tool.sh",
        "#!/bin/sh\nfor arg in \"$@\"; do\n  case \"$arg\" in --invocation) echo \"unknown argument: --invocation\" >&2; exit 2 ;; esac\ndone\necho tool=A\n",
        0o755,
    );
    put(
        &a,
        "recipes/split-probe.yaml",
        &recipe(
            "split-probe",
            "installed A",
            &[(
                "step",
                format!(
                    "echo recipe=A >> {out}\n\"$AMPLIHACK_HOME/amplifier-bundle/tools/split_tool.sh\" >> {out} 2>&1\n"
                ),
            )],
        ),
        0o644,
    );
    let checkout = world.root.join("checkout");
    git_init(&checkout);
    let b = checkout.join("amplifier-bundle");
    put(
        &b,
        "tools/split_tool.sh",
        "#!/bin/sh\n[ \"$1\" = --invocation ] || { echo 'B needs --invocation' >&2; exit 2; }\necho tool=B\n",
        0o755,
    );
    put(
        &b,
        "recipes/split-probe.yaml",
        &recipe(
            "split-probe",
            "checkout B",
            &[(
                "step",
                format!(
                    "echo recipe=B >> {out}\n\"$AMPLIHACK_HOME/amplifier-bundle/tools/split_tool.sh\" --invocation probe >> {out} 2>&1\n"
                ),
            )],
        ),
        0o644,
    );
    (world, checkout)
}

#[test]
fn t1_default_run_inside_a_newer_checkout_uses_the_installed_recipe_and_tool() {
    let runner = runner("t1 default");
    let (world, checkout) = split_world();
    let output = world.run(
        Some(&runner),
        &checkout,
        &["split-probe", "-w", checkout.to_str().unwrap()],
    );
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "run failed:\n{stderr}");
    assert_eq!(world.lines(), ["recipe=A", "tool=A"], "stderr:\n{stderr}");
    assert!(!stderr.contains("unknown argument"), "{stderr}");
    let source = world.installed.join("amplifier-bundle");
    assert_one_bundle(&stderr, &source, "installed");
    assert!(
        stderr.contains(&format!(
            "recipe bundle: {} (unknown); tools: same bundle",
            source.display()
        )),
        "{stderr}"
    );
}

#[test]
fn t1_checkout_choice_uses_the_checkout_recipe_and_tool() {
    let runner = runner("t1 checkout");
    for how in [["--bundle", "checkout"], ["-c", "bundle=checkout"]] {
        let (world, checkout) = split_world();
        let mut args = vec!["split-probe", "-w", checkout.to_str().unwrap()];
        args.extend(how);
        let output = world.run(Some(&runner), &checkout, &args);
        let stderr = text(&output.stderr);
        assert!(output.status.success(), "{how:?} failed:\n{stderr}");
        assert_eq!(world.lines(), ["recipe=B", "tool=B"], "{how:?}:\n{stderr}");
        assert!(!stderr.contains("unknown argument"), "{stderr}");
        assert_one_bundle(&stderr, &checkout.join("amplifier-bundle"), "checkout");
    }
}

#[test]
fn t1_flag_and_key_that_disagree_are_refused_before_any_step() {
    // No runner needed: the choice is settled before the runner is probed.
    let (world, checkout) = split_world();
    let output = world.run(
        None,
        &checkout,
        &[
            "split-probe",
            "-w",
            checkout.to_str().unwrap(),
            "--bundle",
            "installed",
            "-c",
            "bundle=checkout",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr)
            .contains("--bundle installed conflicts with -c bundle=checkout; give one value"),
        "{}",
        text(&output.stderr)
    );
    assert!(world.lines().is_empty(), "no step ran");
}

#[test]
fn t1_an_invalid_bundle_key_is_refused() {
    let (world, checkout) = split_world();
    let output = world.run(
        None,
        &checkout,
        &[
            "split-probe",
            "-w",
            checkout.to_str().unwrap(),
            "-c",
            "bundle=Checkout",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("-c bundle=Checkout is not valid; use installed or checkout"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn t1_the_installed_choice_reports_e9_for_a_recipe_only_the_checkout_has() {
    let (world, checkout) = split_world();
    put(
        &checkout.join("amplifier-bundle"),
        "recipes/only-in-checkout.yaml",
        &recipe("only-in-checkout", "B", &[("step", "true\n".into())]),
        0o644,
    );
    let output = world.run(
        None,
        &checkout,
        &["only-in-checkout", "-w", checkout.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("recipe 'only-in-checkout' not found in the installed bundle at"),
        "{stderr}"
    );
    assert!(stderr.contains("pass --bundle checkout"), "{stderr}");
}

// ---------------------------------------------------------------------- T2

#[test]
fn t2_a_group_writable_checkout_bundle_is_refused_before_any_step() {
    // A group-writable bundle root fails selection, before the runner probe.
    let (world, checkout) = split_world();
    let bundle = checkout.join("amplifier-bundle");
    set_mode(&bundle, 0o775);
    let output = world.run(
        None,
        &checkout,
        &[
            "split-probe",
            "-w",
            checkout.to_str().unwrap(),
            "--bundle",
            "checkout",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(stderr.contains(bundle.to_str().unwrap()), "{stderr}");
    assert!(world.lines().is_empty(), "no step ran");
}

#[test]
fn t2_a_group_writable_file_names_the_file_and_its_mode() {
    let runner = runner("t2 file");
    let (world, checkout) = split_world();
    let bad = checkout.join("amplifier-bundle/tools/split_tool.sh");
    set_mode(&bad, 0o775);
    let output = world.run(
        Some(&runner),
        &checkout,
        &[
            "split-probe",
            "-w",
            checkout.to_str().unwrap(),
            "--bundle",
            "checkout",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "untrusted bundle entry {}: mode 0775 (group or other writable); fix with chmod go-w",
            bad.display()
        )),
        "{stderr}"
    );
    assert!(world.lines().is_empty(), "no step ran:\n{stderr}");
}

// ------------------------------------------------------------------- T3/T4

/// Writes bundle `label` at `bundle`: `identity.sh` prints `<what>=<label>`,
/// sub-recipe `upgrade-sub` and nested recipe `upgrade-nested` use it, and
/// `upgrade-probe` (step 1 given) runs the upgrade, then the tool, the
/// sub-recipe and a nested run.
fn upgrade_bundle(world: &World, bundle: &Path, label: &str, swap: &str) {
    let out = quoted(&world.out());
    let tool = "\"$AMPLIHACK_HOME/amplifier-bundle/tools/identity.sh\"";
    put(
        bundle,
        "tools/identity.sh",
        &format!("#!/bin/sh\necho \"$1={label}\"\n"),
        0o755,
    );
    put(
        bundle,
        "recipes/upgrade-sub.yaml",
        &recipe(
            "upgrade-sub",
            label,
            &[("sub", format!("{tool} sub >> {out}\n"))],
        ),
        0o644,
    );
    put(
        bundle,
        "recipes/upgrade-nested.yaml",
        &recipe(
            "upgrade-nested",
            label,
            &[(
                "nested",
                format!("{tool} nested >> {out}\necho \"state=$AMPLIHACK_STATE_HOME\" >> {out}\n"),
            )],
        ),
        0o644,
    );
    let nested_err = quoted(&world.records.join("nested.stderr"));
    let indent = |body: &str| -> String { body.lines().map(|l| format!("      {l}\n")).collect() };
    let mut main = format!(
        "name: upgrade-probe\ndescription: issue 2528 {label}\nversion: 1.0.0\ncontext: {{}}\nsteps:\n"
    );
    main.push_str("  - id: upgrade\n    type: bash\n    command: |\n");
    main.push_str(&indent(swap));
    main.push_str("  - id: tool\n    type: bash\n    command: |\n");
    main.push_str(&indent(&format!(
        "{tool} tool >> {out}\necho \"state=$AMPLIHACK_STATE_HOME\" >> {out}\n"
    )));
    main.push_str("  - id: sub\n    type: recipe\n    recipe: upgrade-sub\n");
    main.push_str("  - id: nested\n    type: bash\n    command: |\n");
    main.push_str(&indent(&format!(
        "{amp} recipe run upgrade-nested -w {root} > /dev/null 2> {nested_err}\n",
        amp = quoted(&amplihack_bin()),
        root = quoted(&world.root),
    )));
    put(bundle, "recipes/upgrade-probe.yaml", &main, 0o644);
}

fn assert_upgrade_did_not_reach_the_run(world: &World, stderr: &str) {
    let lines = world.lines();
    for expected in ["tool=A", "sub=A", "nested=A"] {
        assert!(
            lines.iter().any(|l| l == expected),
            "missing {expected}: {lines:?}\nstderr:\n{stderr}"
        );
    }
    assert!(
        !lines.iter().any(|l| l.ends_with("=B")),
        "the upgraded bundle reached the run: {lines:?}"
    );
    let nested = fs::read_to_string(world.records.join("nested.stderr")).unwrap_or_default();
    assert!(
        nested.contains("(inherited)"),
        "the nested run reuses the outer snapshot:\n{nested}"
    );
}

#[test]
fn t3_replacing_a_flat_installed_bundle_mid_run_does_not_change_the_run() {
    let runner = runner("t3");
    let world = World::new();
    let staged = world.root.join("staged/amplifier-bundle");
    let installed_bundle = world.installed.join("amplifier-bundle");
    let swap = format!(
        "mv {inst} {old}\nmv {staged} {inst}\n",
        inst = quoted(&installed_bundle),
        old = quoted(&world.root.join("old-A")),
        staged = quoted(&staged),
    );
    upgrade_bundle(&world, &installed_bundle, "A", &swap);
    upgrade_bundle(&world, &staged, "B", "true\n");
    let output = world.run(
        Some(&runner),
        &world.root,
        &["upgrade-probe", "-w", world.root.to_str().unwrap()],
    );
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "run failed:\n{stderr}");
    assert!(
        fs::read_to_string(installed_bundle.join("tools/identity.sh"))
            .unwrap()
            .contains("=B"),
        "the upgrade happened"
    );
    assert_upgrade_did_not_reach_the_run(&world, &stderr);
}

#[test]
fn t4_re_pointing_the_release_link_mid_run_does_not_change_the_run() {
    let runner = runner("t4");
    let world = World::new();
    let releases = world.installed.join("releases");
    for (version, label) in [("1.0.0", "A"), ("2.0.0", "B")] {
        let release = releases.join(version);
        fs::create_dir_all(&release).unwrap();
        fs::write(release.join(".complete"), "").unwrap();
        fs::write(
            release.join(".installed-by"),
            format!("format=1\nversion={version}\nexe=\npid=1\npost_update=0\n"),
        )
        .unwrap();
        set_mode(&release, 0o755);
        let swap = format!(
            "ln -sfn releases/2.0.0/amplifier-bundle {tmp}\nmv -T {tmp} {link}\n",
            tmp = quoted(&world.installed.join("amplifier-bundle.next")),
            link = quoted(&world.installed.join("amplifier-bundle")),
        );
        upgrade_bundle(&world, &release.join("amplifier-bundle"), label, &swap);
    }
    set_mode(&releases, 0o755);
    symlink(
        "releases/1.0.0/amplifier-bundle",
        world.installed.join("amplifier-bundle"),
    )
    .unwrap();

    let output = world.run(
        Some(&runner),
        &world.root,
        &["upgrade-probe", "-w", world.root.to_str().unwrap()],
    );
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "run failed:\n{stderr}");
    assert_eq!(
        fs::read_link(world.installed.join("amplifier-bundle")).unwrap(),
        PathBuf::from("releases/2.0.0/amplifier-bundle"),
        "the link was re-pointed"
    );
    assert_upgrade_did_not_reach_the_run(&world, &stderr);
    let source = releases.join("1.0.0/amplifier-bundle");
    assert_one_bundle(&stderr, &source, "installed");
    let early = records(&stderr)
        .into_iter()
        .find(|r| r["event"] == "early")
        .unwrap();
    assert_eq!(early["recipe_bundle"]["version"], "1.0.0");
    assert!(
        stderr.contains(&format!(
            "recipe bundle: {} (1.0.0); tools: same bundle",
            source.display()
        )),
        "{stderr}"
    );
}

// ---------------------------------------------------------------------- T6

#[test]
fn t6_steps_see_the_real_state_root_not_the_snapshot() {
    let runner = runner("t6");
    let world = World::new();
    let bundle = world.installed.join("amplifier-bundle");
    upgrade_bundle(&world, &bundle, "A", "true\n");
    let output = world.run(
        Some(&runner),
        &world.root,
        &["upgrade-probe", "-w", world.root.to_str().unwrap()],
    );
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "run failed:\n{stderr}");
    let states: Vec<String> = world
        .lines()
        .into_iter()
        .filter_map(|l| l.strip_prefix("state=").map(str::to_owned))
        .collect();
    assert_eq!(states.len(), 2, "outer and nested steps: {states:?}");
    for state in states {
        // AMPLIHACK_HOME is the installed home (not a checkout): the state
        // root that applied before re-pointing.
        assert_eq!(Path::new(&state), world.installed, "{stderr}");
        // Not inside the work root, where the snapshot lives. (The temp
        // root itself may sit under some other `runtime/` folder.)
        assert!(!Path::new(&state).starts_with(&world.work), "{state}");
    }
    let snapshot = assert_one_bundle(&stderr, &bundle, "installed").expect("snapshot recorded");
    // `<work root>/bundle-pins/<key>`: a folder named by the bundle's
    // contents, outside the run's own `runtime/` folder.
    assert_eq!(
        snapshot.parent(),
        Some(world.work.join("bundle-pins").as_path()),
        "{}",
        snapshot.display()
    );
    let key = snapshot
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    assert!(
        key.len() == 24 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "{key}"
    );
    assert!(snapshot.starts_with(&world.work), "{}", snapshot.display());
    assert!(
        !snapshot.exists(),
        "the snapshot is removed when the run ends"
    );
}

// ------------------------------------------------------- auto-drive (D5, D7)

/// The assets `LaunchBundle::prepare` requires before an auto-drive run.
const AUTODRIVE_ASSETS: &[&str] = &[
    "skills/merge-ready/SKILL.md",
    "skills/crusty-old-engineer/SKILL.md",
    "skills/merge-ready/pr-description-template.md",
    "recipes/loop-health-evaluator.yaml",
    "recipes/autodrive-finalize.yaml",
    "skills/auto-drive-to-merge/evidence-contract.md",
    "tools/autodrive_loop.sh",
    "tools/autodrive_merge_message.sh",
    "tools/autodrive_expected_checks.sh",
];

#[test]
fn autodrive_run_turns_default_search_off_and_nested_runs_inherit_the_snapshot() {
    let runner = runner("auto-drive");
    let world = World::new();
    // A wrapper that records the runner's arguments, then runs it.
    let args_file = world.records.join("runner-args.txt");
    let wrapper = world.root.join("wrapper/recipe-runner-rs");
    fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nexec {} \"$@\"\n",
            quoted(&args_file),
            quoted(&runner)
        ),
    )
    .unwrap();
    set_mode(&wrapper, 0o755);

    let checkout = world.root.join("checkout");
    git_init(&checkout);
    let bundle = checkout.join("amplifier-bundle");
    for asset in AUTODRIVE_ASSETS {
        let (body, mode) = if asset.ends_with(".sh") {
            ("#!/bin/sh\n".to_owned(), 0o755)
        } else if asset.ends_with(".yaml") {
            let name = Path::new(asset).file_stem().unwrap().to_str().unwrap();
            (recipe(name, "asset", &[("noop", "true\n".into())]), 0o644)
        } else {
            ("# asset\n".to_owned(), 0o644)
        };
        put(&bundle, asset, &body, mode);
    }
    put(&bundle, "agents/core/builder.md", "# builder\n", 0o644);
    let out = quoted(&world.out());
    put(
        &bundle,
        "recipes/nested-2528.yaml",
        &recipe(
            "nested-2528",
            "nested",
            &[("n", format!("echo nested=ran >> {out}\n"))],
        ),
        0o644,
    );
    let nested_err = quoted(&world.records.join("nested.stderr"));
    put(
        &bundle,
        "recipes/autodrive-2528-probe.yaml",
        &recipe(
            "autodrive-2528-probe",
            "auto-drive",
            &[(
                "env",
                format!(
                    "echo \"home=$AMPLIHACK_HOME\" >> {out}\n\
                     echo \"marker=$AMPLIHACK_RUN_BUNDLE\" >> {out}\n\
                     echo \"choice=$AMPLIHACK_RUN_BUNDLE_CHOICE\" >> {out}\n\
                     {amp} recipe run nested-2528 -w {root} > /dev/null 2> {nested_err}\n",
                    amp = quoted(&amplihack_bin()),
                    root = quoted(&world.root),
                ),
            )],
        ),
        0o644,
    );

    let output = world.run(
        Some(&wrapper),
        &checkout,
        &[
            "autodrive-2528-probe",
            "--bundle",
            "checkout",
            "-w",
            checkout.to_str().unwrap(),
        ],
    );
    let stderr = text(&output.stderr);
    let nested = fs::read_to_string(world.records.join("nested.stderr")).unwrap_or_default();
    assert!(
        output.status.success(),
        "run failed:\n{stderr}\nnested:\n{nested}"
    );

    // The outer runner gets the launch copy's agents and no default search.
    let args = fs::read_to_string(&args_file).unwrap_or_default();
    let outer: Vec<&str> = args.lines().collect();
    assert!(outer.contains(&"--no-default-search"), "{args}");
    let agent_dir = outer
        .iter()
        .position(|a| *a == "--agent-dir")
        .map(|i| outer[i + 1])
        .unwrap_or_else(|| panic!("no --agent-dir: {args}"));
    assert!(
        agent_dir.contains("autodrive-invocation-")
            && agent_dir.ends_with("amplifier-bundle/agents"),
        "{agent_dir}"
    );

    // Steps: AMPLIHACK_HOME is the launch copy; the marker is the snapshot.
    let lines = world.lines();
    let value = |key: &str| {
        lines
            .iter()
            .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_owned))
            .unwrap_or_else(|| panic!("no {key}: {lines:?}\n{stderr}"))
    };
    assert!(value("home").contains("autodrive-invocation-"), "{lines:?}");
    let snapshot = assert_one_bundle(&stderr, &fs::canonicalize(&bundle).unwrap(), "checkout")
        .expect("snapshot recorded");
    assert_eq!(Path::new(&value("marker")), snapshot, "{lines:?}");
    assert_eq!(value("choice"), "checkout");

    // The nested run reuses the snapshot: no second copy, same choice.
    assert!(
        lines.iter().any(|l| l == "nested=ran"),
        "{lines:?}\n{nested}"
    );
    assert!(nested.contains("(inherited)"), "{nested}");
    for record in records(&nested) {
        if record["event"] == "early" {
            assert_eq!(record["recipe_bundle"]["choice"], "checkout", "{record}");
            assert_eq!(record["recipe_bundle"]["inherited"], true, "{record}");
        }
    }
}

// ---------------------------------------------------------------------- R2

/// The early record's `recipe_bundle` of one attempt.
fn early_bundle(stderr: &str) -> serde_json::Value {
    records(stderr)
        .into_iter()
        .find(|r| r["event"] == "early")
        .map(|r| r["recipe_bundle"].clone())
        .unwrap_or_else(|| panic!("no early record:\n{stderr}"))
}

#[test]
fn r2_a_resumed_run_pins_the_same_bundle_with_the_same_choice() {
    runner("r2");
    // The recovery fixture stops a run at a usage limit in its second step;
    // the next launch resumes it. No timing is involved.
    let f = crate::issue_1741_fixture::Fixture::new();
    let first = f.stop_at("design");
    f.mode("design", "success");
    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        crate::issue_1741_fixture::describe(&second.output)
    );
    second.assert_resumed(&first, "design");

    let before = early_bundle(&text(&first.output.stderr));
    let after = early_bundle(&text(&second.output.stderr));
    for key in ["source", "version", "choice"] {
        assert_eq!(before[key], after[key], "{key}: {before} vs {after}");
    }
    assert_eq!(after["choice"], "installed");
    assert_eq!(after["inherited"], false);
    // Both launches pin the unchanged bundle at the same content-keyed path,
    // so the resumed run's runner arguments and recovery fingerprints match
    // the first run's (#1741). The snapshot is removed when each launch ends.
    assert_eq!(
        before["snapshot"], after["snapshot"],
        "same snapshot path: {before} vs {after}"
    );
    let snapshot = PathBuf::from(after["snapshot"].as_str().expect("snapshot"));
    assert_eq!(
        snapshot.parent(),
        Some(f.work_root.join("bundle-pins").as_path()),
        "{}",
        snapshot.display()
    );
    assert!(!snapshot.exists(), "{}", snapshot.display());
}

// ---------------------------------------------------------------------- B1

/// `split_world()` plus, in both bundles, `split-parent` (writes
/// `recipe=<X>`, then calls sub-recipe `split-sub`) and `split-sub` (writes
/// `sub=<X>`, then runs that bundle's `split_tool.sh` the way only that
/// bundle's tool accepts). A run that mixes bundles writes both letters or
/// fails at the tool.
fn path_world() -> (World, PathBuf) {
    let (world, checkout) = split_world();
    let out = quoted(&world.out());
    let tool = "\"$AMPLIHACK_HOME/amplifier-bundle/tools/split_tool.sh\"";
    for (bundle, label, tool_args) in [
        (world.installed.join("amplifier-bundle"), "A", ""),
        (
            checkout.join("amplifier-bundle"),
            "B",
            " --invocation probe",
        ),
    ] {
        let mut parent = recipe(
            "split-parent",
            label,
            &[("parent", format!("echo recipe={label} >> {out}\n"))],
        );
        parent.push_str("  - id: sub\n    type: recipe\n    recipe: split-sub\n");
        put(&bundle, "recipes/split-parent.yaml", &parent, 0o644);
        put(
            &bundle,
            "recipes/split-sub.yaml",
            &recipe(
                "split-sub",
                label,
                &[(
                    "sub",
                    format!("echo sub={label} >> {out}\n{tool}{tool_args} >> {out} 2>&1\n"),
                )],
            ),
            0o644,
        );
    }
    (world, checkout)
}

#[test]
fn b1_path_recipe_runs_from_one_bundle_or_is_refused() {
    let runner = runner("b1 path");
    let rel = "amplifier-bundle/recipes/split-parent.yaml";

    // (a) A checkout path with the default (installed) bundle: refused
    // before any step, with advice that names --bundle checkout and the name.
    let (world, checkout) = path_world();
    let output = world.run(
        Some(&runner),
        &checkout,
        &[rel, "-w", checkout.to_str().unwrap()],
    );
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(world.lines().is_empty(), "no step ran: {:?}", world.lines());
    assert!(stderr.contains("belongs to the bundle at"), "{stderr}");
    assert!(stderr.contains("--bundle checkout"), "{stderr}");
    assert!(
        stderr.contains("amplihack recipe run split-parent"),
        "{stderr}"
    );

    // (b) The same path with --bundle checkout: recipe, sub-recipe and tool
    // all come from the checkout bundle.
    let (world, checkout) = path_world();
    let output = world.run(
        Some(&runner),
        &checkout,
        &[
            rel,
            "--bundle",
            "checkout",
            "-w",
            checkout.to_str().unwrap(),
        ],
    );
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "checkout path failed:\n{stderr}");
    assert_eq!(
        world.lines(),
        ["recipe=B", "sub=B", "tool=B"],
        "stderr:\n{stderr}"
    );
    assert!(!stderr.contains("unknown argument"), "{stderr}");
    let source = checkout.join("amplifier-bundle");
    assert_one_bundle(&stderr, &source, "checkout");
    assert!(stderr.contains("; tools: same bundle"), "{stderr}");

    // (c) A path into the installed bundle, run from inside the checkout:
    // everything comes from the installed bundle.
    let (world, checkout) = path_world();
    let installed_path = world
        .installed
        .join("amplifier-bundle/recipes/split-parent.yaml");
    let output = world.run(
        Some(&runner),
        &checkout,
        &[
            installed_path.to_str().unwrap(),
            "-w",
            checkout.to_str().unwrap(),
        ],
    );
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "installed path failed:\n{stderr}");
    assert_eq!(
        world.lines(),
        ["recipe=A", "sub=A", "tool=A"],
        "stderr:\n{stderr}"
    );
    assert_one_bundle(
        &stderr,
        &world.installed.join("amplifier-bundle"),
        "installed",
    );
}

#[test]
fn b1_a_missing_recipe_path_is_e15_and_exits_1_before_the_runner() {
    // No runner: the path is checked before the runner is probed.
    let (world, checkout) = split_world();
    let given = "amplifier-bundle/recipes/absent.yaml";
    let output = world.run(None, &checkout, &[given, "-w", checkout.to_str().unwrap()]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!("recipe {given} not found: ")),
        "{stderr}"
    );
    assert!(world.lines().is_empty(), "no step ran");
}

// --------------------------------------------------------------------- T6b

#[test]
fn t6b_steps_find_the_git_identity_under_a_non_default_amplihack_home() {
    let runner = runner("t6b");
    let world = World::new();
    let bundle = world.installed.join("amplifier-bundle");
    let helper =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/tools/git-identity.sh");
    put(
        &bundle,
        "tools/git-identity.sh",
        &fs::read_to_string(&helper).expect("read the shipped git-identity.sh"),
        0o755,
    );
    // The identity lives under the caller's AMPLIHACK_HOME, which is not a
    // checkout and so is the state root. HOME has no config.
    let config = world.installed.join("config");
    fs::write(
        &config,
        "[git_identity]\nauthor_name = State Home Tester\nauthor_email = state-home@example.com\n",
    )
    .unwrap();
    set_mode(&config, 0o644);
    let out = quoted(&world.out());
    put(
        &bundle,
        "recipes/identity-probe.yaml",
        &recipe(
            "identity-probe",
            "t6b",
            &[(
                "identity",
                format!(
                    "echo \"home=$AMPLIHACK_HOME\" >> {out}\n\
                     . \"$AMPLIHACK_HOME/amplifier-bundle/tools/git-identity.sh\"\n\
                     amplihack_resolve_git_identity >> {out} || echo identity=failed >> {out}\n"
                ),
            )],
        ),
        0o644,
    );
    // A project with no repo-local identity and no remote, so the lookup
    // can only succeed from the config file; any git command the helper runs
    // on the way is in a repository the run can attribute.
    let project = world.root.join("project");
    git_init(&project);
    let output = world.run(
        Some(&runner),
        &project,
        &["identity-probe", "-w", project.to_str().unwrap()],
    );
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "run failed:\n{stderr}");
    let lines = world.lines();
    // The step ran from the snapshot, so AMPLIHACK_HOME/config is not where
    // the identity is.
    let home = lines
        .iter()
        .find_map(|l| l.strip_prefix("home="))
        .unwrap_or_else(|| panic!("no home line: {lines:?}\n{stderr}"));
    assert_ne!(Path::new(home), world.installed, "{lines:?}");
    assert!(
        lines.iter().any(|l| l
            == "State Home Tester\tstate-home@example.com\tState Home Tester\tstate-home@example.com"),
        "identity not found under the state home: {lines:?}\n{stderr}"
    );
}

// ---------------------------------------------------------------------- B3

/// B3: `autodrive_loop.sh` starts each round by the path it builds,
/// `<launch copy>/amplifier-bundle/recipes/<round>.yaml`, because the round
/// recipe must not be taken from the worktree under review (#1528, #1543)
/// and because the copy's `autodrive-*` recipes carry the run's
/// authorisation. The run must accept that path: it names the run's own
/// bundle, which is what the one-bundle rule asks for, not another bundle.
///
/// The step here starts the round exactly as the loop does. The nested run
/// reuses the outer run's snapshot, and its recipe, sub-recipes and tools all
/// come from it.
#[test]
fn b3_a_step_starts_a_nested_round_by_the_path_into_its_own_bundle_copy() {
    let runner = runner("b3");
    let world = World::new();
    let checkout = world.root.join("checkout");
    git_init(&checkout);
    let bundle = checkout.join("amplifier-bundle");
    for asset in AUTODRIVE_ASSETS {
        let (body, mode) = if asset.ends_with(".sh") {
            ("#!/bin/sh\n".to_owned(), 0o755)
        } else if asset.ends_with(".yaml") {
            let name = Path::new(asset).file_stem().unwrap().to_str().unwrap();
            (recipe(name, "asset", &[("noop", "true\n".into())]), 0o644)
        } else {
            ("# asset\n".to_owned(), 0o644)
        };
        put(&bundle, asset, &body, mode);
    }
    put(&bundle, "agents/core/builder.md", "# builder\n", 0o644);

    let out = quoted(&world.out());
    // The round, as a crusty or merge round is: an `autodrive-` recipe that
    // the trust checks rewrite, calling a tool from the same bundle.
    put(
        &bundle,
        "tools/round_tool.sh",
        "#!/bin/sh\necho tool=round\n",
        0o755,
    );
    put(
        &bundle,
        "recipes/autodrive-2528-round.yaml",
        &recipe(
            "autodrive-2528-round",
            "round",
            &[(
                "round",
                format!(
                    "echo round=ran >> {out}\n\
                     \"$AMPLIHACK_HOME/amplifier-bundle/tools/round_tool.sh\" >> {out} 2>&1\n"
                ),
            )],
        ),
        0o644,
    );

    let round_err = quoted(&world.records.join("round.stderr"));
    put(
        &bundle,
        "recipes/autodrive-2528-loop.yaml",
        &recipe(
            "autodrive-2528-loop",
            "loop",
            &[(
                "start-round",
                format!(
                    "round=\"$AMPLIHACK_HOME/amplifier-bundle/recipes/autodrive-2528-round.yaml\"\n\
                     echo \"round_path=$round\" >> {out}\n\
                     {amp} recipe run \"$round\" -w {root} > /dev/null 2> {round_err}\n\
                     echo \"round_rc=$?\" >> {out}\n",
                    amp = quoted(&amplihack_bin()),
                    root = quoted(&world.root),
                ),
            )],
        ),
        0o644,
    );

    let output = world.run(
        Some(&runner),
        &checkout,
        &[
            "autodrive-2528-loop",
            "--bundle",
            "checkout",
            "-w",
            checkout.to_str().unwrap(),
        ],
    );
    let stderr = text(&output.stderr);
    let round = fs::read_to_string(world.records.join("round.stderr")).unwrap_or_default();
    assert!(
        output.status.success(),
        "run failed:\n{stderr}\nround:\n{round}"
    );
    let lines = world.lines();

    // The round started, from the path the loop builds, and exited 0.
    assert!(
        !round.contains("belongs to the bundle at"),
        "a path into the run's own bundle copy is not another bundle:\n{round}"
    );
    assert!(
        lines.iter().any(|l| l == "round_rc=0"),
        "the round did not run: {lines:?}\n{round}"
    );
    assert!(lines.iter().any(|l| l == "round=ran"), "{lines:?}\n{round}");

    // The path it was started by lies inside the auto-drive launch copy.
    let round_path = lines
        .iter()
        .find_map(|l| l.strip_prefix("round_path="))
        .unwrap_or_else(|| panic!("no round_path: {lines:?}\n{stderr}"));
    assert!(
        round_path.contains("autodrive-invocation-")
            && round_path.ends_with("amplifier-bundle/recipes/autodrive-2528-round.yaml"),
        "{round_path}"
    );

    // One bundle: the round reuses the outer run's snapshot for its recipe,
    // its sub-recipes and its tools, and makes no second copy.
    let snapshot = assert_one_bundle(&stderr, &fs::canonicalize(&bundle).unwrap(), "checkout")
        .expect("snapshot recorded");
    assert!(
        lines.iter().any(|l| l == "tool=round"),
        "the round's tool came from the one bundle: {lines:?}\n{round}"
    );
    assert!(round.contains("(inherited)"), "{round}");
    let mut saw_early = false;
    for record in records(&round) {
        if record["event"] == "early" {
            saw_early = true;
            assert_eq!(record["recipe_bundle"]["inherited"], true, "{record}");
            assert_eq!(record["recipe_bundle"]["choice"], "checkout", "{record}");
            assert_eq!(
                record["recipe_bundle"]["snapshot"],
                snapshot.to_str().unwrap(),
                "{record}"
            );
            assert_eq!(record["recipe_bundle"], record["tool_bundle"], "{record}");
        }
    }
    assert!(saw_early, "no early record from the round:\n{round}");
}

// ------------------------------------------------------------- C1: the runner

/// C1 (#2658, #2687): the real runner these tests drive is the receipt-checked
/// build at the revision pinned in `claude-plugin/recipe-runner.rev`, resolved
/// by the one shared helper. It is never whatever `recipe-runner-rs` the host
/// happens to have on `PATH`.
///
/// The #2528 tests drive real recipe runs, so the binary this returns executes
/// the fixtures' shell steps. Walking `PATH` for it makes the binary under
/// test depend on the ambient environment: any earlier `PATH` entry holding a
/// file of that name decides what runs. `pinned_runner` instead puts every
/// source through the install-receipt check and has no fallback, so a source
/// that fails it stops the test rather than quietly supplying a substitute.
/// (The receipt check is a consistency check on a Cargo install receipt. It
/// is not a signature and not proof of provenance.)
///
/// Two separate claims, because either alone can be met by the wrong code:
/// the helper delegates to the shared resolution, and the path it yields
/// belongs to one of the three sources that resolution allows.
#[test]
fn c1_the_runner_is_the_pinned_one_and_never_a_path_lookup() {
    let resolved = runner("c1");
    assert!(
        resolved.is_absolute(),
        "the runner must be an absolute path: {}",
        resolved.display()
    );

    // 1. The same binary the shared helper resolves. A local copy of the
    //    lookup drifts from the pin; delegation cannot.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let shared = crate::pinned_runner::pinned_runner(&repo_root);
    assert_eq!(
        canonical(&resolved),
        canonical(&shared),
        "the #2528 runner must be the shared pinned runner ({}), not a lookup of its own ({})",
        shared.display(),
        resolved.display()
    );

    // 2. One of the sources the pin allows: either override, or the build
    //    cached under this test target's own temporary directory. A binary
    //    found by scanning `PATH` matches none of them.
    let overrides: Vec<PathBuf> = ["RECIPE_RUNNER_QA_BINARY", "RECIPE_RUNNER_RS_PATH"]
        .iter()
        .filter_map(|name| std::env::var_os(name).map(PathBuf::from))
        .collect();
    let cache = canonical(&Path::new(env!("CARGO_TARGET_TMPDIR")).join("pinned-recipe-runner"));
    let from_override = overrides
        .iter()
        .any(|given| canonical(given) == canonical(&resolved));
    let from_cache = canonical(&resolved).starts_with(&cache);
    assert!(
        from_override || from_cache,
        "the runner at {} comes from neither RECIPE_RUNNER_QA_BINARY nor \
         RECIPE_RUNNER_RS_PATH ({overrides:?}) nor the pinned build cached under {}; \
         a binary found on PATH is not the pinned runner",
        resolved.display(),
        cache.display()
    );
}

/// Canonical form where the path resolves, the path itself otherwise, so a
/// comparison fails closed instead of panicking on a path that is simply
/// absent.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
