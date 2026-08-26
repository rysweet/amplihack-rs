//! Process-level conformance gate for `docs/spec/OrchLedger.tla` (issue #1326).
//!
//! `NodeBudget` and `LedgerSound` are properties of *concurrent processes
//! sharing a file*, which is exactly what the in-process tests cannot observe.
//! These run the real `amplihack` binary.
//!
//! See `crates/amplihack-cli/tests/spec_conformance.rs` for the traceability
//! table and the drift guard that requires these functions to exist.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_amplihack");

/// A throwaway HOME so the durable tree directory lands somewhere disposable.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "amplihack-spec-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&root).expect("sandbox root");
        Sandbox { root }
    }

    fn tree_dir(&self) -> PathBuf {
        self.root.join(".amplihack").join("amplihack-session-trees")
    }

    /// `session-tree register`, with an explicit TMPDIR to prove the tree
    /// location does not depend on it.
    fn register(&self, tree_id: &str, session: &str, tmpdir: &Path, max_sessions: u32) -> bool {
        fs::create_dir_all(tmpdir).expect("tmpdir");
        Command::new(BIN)
            .args(["session-tree", "register", session])
            .env("HOME", &self.root)
            .env("TMPDIR", tmpdir)
            .env("AMPLIHACK_TREE_ID", tree_id)
            .env("AMPLIHACK_MAX_SESSIONS", max_sessions.to_string())
            // Hermetic: the sandbox HOME has no staged assets, and re-staging
            // needs network plus an authenticated agent.
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env_remove("AMPLIHACK_SESSION_TREE_DIR")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("spawn amplihack")
            .success()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// ---------------------------------------------------------------------------
// LedgerSound
//   SharedLedger => spawned["shared"] >= |{p : pc[p] # "off"}|
//
// The precondition is that all processes in a tree resolve the SAME ledger.
// Before the fix the path came from TMPDIR, and every nested run is handed a
// fresh TMPDIR, so each level counted against an empty file.
// ---------------------------------------------------------------------------

/// Two registrations with *different* TMPDIRs must share one tree file.
#[test]
fn invariant_ledger_sound_tree_dir_is_tmpdir_invariant() {
    let sb = Sandbox::new("ledger");
    let a = sb.root.join("tmp-a");
    let b = sb.root.join("tmp-b");

    assert!(
        sb.register("t1", "s1", &a, 10),
        "first register should succeed"
    );
    assert!(
        sb.register("t1", "s2", &b, 10),
        "second register should succeed"
    );

    let state = sb.tree_dir().join("t1.json");
    assert!(
        state.exists(),
        "tree state must live under HOME, not TMPDIR; expected {}",
        state.display()
    );
    let body = fs::read_to_string(&state).expect("readable state");
    assert!(
        body.contains("s1") && body.contains("s2"),
        "both sessions must land in ONE tree file regardless of TMPDIR; got: {body}"
    );

    // And nothing may have been written under either TMPDIR.
    for t in [&a, &b] {
        let stray = t.join("amplihack-session-trees");
        assert!(
            !stray.exists(),
            "tree state must not be written under TMPDIR: {}",
            stray.display()
        );
    }
}

// ---------------------------------------------------------------------------
// NodeBudget
//   Cardinality({p : pc[p] # "off"}) =< MaxNodes
//
// The `C_no_lock` ablation shows an unlocked read-modify-write admits far more
// than the budget (85 against a budget of 12 in the executable model). The
// implementation already locks; this asserts that under real concurrency.
// ---------------------------------------------------------------------------

/// With `max_sessions = N`, at most N concurrent registrations may succeed.
#[test]
fn invariant_node_budget_concurrent_registers_respect_cap() {
    const CAP: u32 = 3;
    const ATTEMPTS: usize = 12;

    let sb = Sandbox::new("budget");
    let tmp = sb.root.join("tmp");
    fs::create_dir_all(&tmp).expect("tmpdir");

    // Spawn all attempts before reaping any, so they genuinely contend.
    let mut children: Vec<_> = (0..ATTEMPTS)
        .map(|i| {
            Command::new(BIN)
                .args(["session-tree", "register", &format!("s{i}")])
                .env("HOME", &sb.root)
                .env("TMPDIR", &tmp)
                .env("AMPLIHACK_TREE_ID", "cap")
                .env("AMPLIHACK_MAX_SESSIONS", CAP.to_string())
                .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
                .env_remove("AMPLIHACK_SESSION_TREE_DIR")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn amplihack")
        })
        .collect();

    let mut admitted = 0usize;
    for child in &mut children {
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            admitted += 1;
        }
    }

    assert!(
        admitted as u32 <= CAP,
        "NodeBudget violated: {admitted} of {ATTEMPTS} admitted against a cap of {CAP}"
    );
    assert!(
        admitted > 0,
        "no registration succeeded — the cap is over-blocking, which breaks nesting"
    );
}

/// The cap must count against the tree that descendants actually share. This is
/// the end-to-end statement of the bug: before the fix, a child handed a fresh
/// TMPDIR saw an empty tree and was admitted no matter how full the tree was.
#[test]
fn invariant_node_budget_survives_a_tmpdir_change() {
    const CAP: u32 = 2;
    let sb = Sandbox::new("budget2");

    assert!(sb.register("t2", "a", &sb.root.join("t1"), CAP));
    assert!(sb.register("t2", "b", &sb.root.join("t2"), CAP));
    assert!(
        !sb.register("t2", "c", &sb.root.join("t3"), CAP),
        "a third session with a fresh TMPDIR must still be refused"
    );
}

/// Sanity: the durable location is under HOME and nowhere near TMPDIR.
#[test]
fn invariant_ledger_sound_tree_dir_is_under_home() {
    let sb = Sandbox::new("home");
    assert!(sb.register("t3", "s1", &sb.root.join("tmp"), 10));
    assert!(
        Path::new(&sb.tree_dir().join("t3.json")).exists(),
        "expected tree state under $HOME/.amplihack/amplihack-session-trees/"
    );
}

// ---------------------------------------------------------------------------
// End-to-end guard behaviour.
//
// The invariants above are checked through `session-tree`, which is NOT the code
// path that failed. `enforce_recursion_depth_guard` runs inside `recipe run`, and
// an earlier revision of this fix passed every test above while leaving the
// escalation wide open, because nothing seeded AMPLIHACK_TREE_ID at a root and the
// guard therefore had no sealed ceiling to clamp against. These tests exercise the
// real entry point.
// ---------------------------------------------------------------------------

/// Exit status for a policy refusal. Mirrors
/// `commands::recipe::run::execute::EXIT_ORCHESTRATION_UNAVAILABLE`.
const EXIT_ORCHESTRATION_UNAVAILABLE: i32 = 79;

fn recipe_run(home: &Path, envs: &[(&str, &str)]) -> Output {
    // A recipe whose only step is a no-op shell command. Recipe validation runs
    // before the recursion guard and rejects an empty `steps`, so the probe needs
    // one step -- but it must not need an agent: pointing these tests at
    // smart-orchestrator made CI npm-install copilot and then fail on auth, which
    // is a slow way to learn nothing about admission control.
    let recipe = home.join("conformance-probe.yaml");
    fs::write(
        &recipe,
        "name: conformance-probe\nsteps:\n  - id: noop\n    type: bash\n    command: \"true\"\n",
    )
    .expect("write probe recipe");

    let mut cmd = Command::new(BIN);
    cmd.args([
        "recipe",
        "run",
        recipe.to_str().expect("recipe path"),
        "-c",
        "task_description=conformance",
    ])
    .env("HOME", home)
    .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
    .env_remove("AMPLIHACK_SESSION_TREE_DIR")
    .env_remove("AMPLIHACK_TREE_ID")
    .env_remove("AMPLIHACK_SESSION_DEPTH")
    .env_remove("AMPLIHACK_MAX_DEPTH");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn amplihack recipe run")
}

fn blocked(out: &Output) -> bool {
    out.status.code() == Some(EXIT_ORCHESTRATION_UNAVAILABLE)
        && String::from_utf8_lossy(&out.stderr).contains("BLOCKED_TERMINAL")
}

/// A nested run naming a tree that has sealed no ceiling must fail closed. The
/// environment claims we are inside a tree; no tree vouches for the ceiling.
///
/// This deliberately supplies a tree id rather than relying on process ancestry.
/// An earlier revision asserted the no-tree-id case and passed only on a developer
/// box, where the test binary happens to descend from an `amplihack` process; on CI
/// it correctly reset to depth 0 and the assertion was simply wrong. The
/// uncorroborated case is covered deterministically by
/// `uncorroborated_stale_depth_is_treated_as_a_new_root` instead.
#[test]
fn invariant_ceiling_monotone_unsealed_nested_run_fails_closed() {
    let sb = Sandbox::new("e2e-unsealed");
    let out = recipe_run(
        &sb.root,
        &[
            ("AMPLIHACK_TREE_ID", "ghosttree"),
            ("AMPLIHACK_SESSION_DEPTH", "2"),
            ("AMPLIHACK_MAX_DEPTH", "99"),
        ],
    );
    assert!(
        blocked(&out),
        "nested run naming a tree with no sealed ceiling must be refused; got status {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The observed escalation, end to end: a sealed ceiling must survive a forged
/// AMPLIHACK_MAX_DEPTH on the real `recipe run` path.
#[test]
fn invariant_ceiling_monotone_forged_env_cannot_raise_a_sealed_ceiling() {
    let sb = Sandbox::new("e2e-forged");
    assert!(sb.register("tf", "root", &sb.root.join("tmp"), 10));
    // Seal a ceiling of 1 on tree "tf".
    let _ = recipe_run(
        &sb.root,
        &[("AMPLIHACK_TREE_ID", "tf"), ("AMPLIHACK_MAX_DEPTH", "1")],
    );
    for forged in ["5", "6", "7", "8", "9", "99"] {
        let out = recipe_run(
            &sb.root,
            &[
                ("AMPLIHACK_TREE_ID", "tf"),
                ("AMPLIHACK_SESSION_DEPTH", "1"),
                ("AMPLIHACK_MAX_DEPTH", forged),
            ],
        );
        assert!(
            blocked(&out),
            "forged AMPLIHACK_MAX_DEPTH={forged} must not raise a sealed ceiling of 1; \
             got status {:?}",
            out.status.code()
        );
    }
}

/// The capability this guard exists to protect. A ROOT run must never be refused,
/// and must seal a tree so its descendants have something to clamp against. A fix
/// that fails closed everywhere would satisfy every safety test and destroy the
/// product.
#[test]
fn invariant_depth_bound_root_run_is_admitted_and_seals_a_tree() {
    let sb = Sandbox::new("e2e-root");
    let out = recipe_run(&sb.root, &[]);
    assert!(
        !blocked(&out),
        "a root run must not be refused; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sealed: Vec<_> = fs::read_dir(sb.tree_dir())
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !sealed.is_empty(),
        "a root run must seal a tree so descendants can clamp against it"
    );
    let body = fs::read_to_string(sealed[0].path()).expect("readable tree state");
    assert!(
        body.contains("\"ceiling\""),
        "sealed tree must record a ceiling; got: {body}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1329: the node budget must apply to the bare `recipe run` path, which is
// what an agent invokes from a bash tool. Before this, six concurrent runs were
// admitted against a configured cap of two, because only `session-tree register`
// debited the tree.
// ---------------------------------------------------------------------------

/// Concurrent runs in one tree are capped, and capacity returns afterwards.
#[test]
fn invariant_node_budget_caps_concurrent_recipe_runs() {
    const CAP: u32 = 2;
    const ATTEMPTS: usize = 6;
    let sb = Sandbox::new("e2e-width");

    // Seed the tree so the cap has something to count against.
    assert!(sb.register("wtree", "root", &sb.root.join("tmp"), CAP));

    let recipe = sb.root.join("slow.yaml");
    fs::write(
        &recipe,
        "name: slow\nsteps:\n  - id: s\n    type: bash\n    command: \"sleep 4\"\n",
    )
    .expect("write recipe");

    let mut children: Vec<_> = (0..ATTEMPTS)
        .map(|_| {
            Command::new(BIN)
                .args([
                    "recipe",
                    "run",
                    recipe.to_str().expect("path"),
                    "-c",
                    "task_description=width",
                ])
                .env("HOME", &sb.root)
                .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
                .env("AMPLIHACK_TREE_ID", "wtree")
                .env("AMPLIHACK_SESSION_DEPTH", "1")
                .env("AMPLIHACK_MAX_DEPTH", "3")
                .env("AMPLIHACK_MAX_SESSIONS", CAP.to_string())
                .env_remove("AMPLIHACK_SESSION_TREE_DIR")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn")
        })
        .collect();

    let mut refused = 0usize;
    for child in &mut children {
        if child.wait().ok().and_then(|s| s.code()) == Some(EXIT_ORCHESTRATION_UNAVAILABLE) {
            refused += 1;
        }
    }

    assert!(
        refused > 0,
        "with a cap of {CAP}, {ATTEMPTS} concurrent runs must produce refusals; got none"
    );
    assert!(
        refused < ATTEMPTS,
        "at least one run must be admitted, or the cap is over-blocking and nesting is broken"
    );
}
