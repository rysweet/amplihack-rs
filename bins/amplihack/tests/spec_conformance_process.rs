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
use std::process::{Command, Stdio};

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

    assert!(sb.register("t1", "s1", &a, 10), "first register should succeed");
    assert!(sb.register("t1", "s2", &b, 10), "second register should succeed");

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
