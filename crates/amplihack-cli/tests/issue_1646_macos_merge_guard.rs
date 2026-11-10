//! Issue #1646: the `gh` merge guard establishes the same five facts about its
//! parent on macOS, where there is no `/proc`.
//!
//! Behaviour tests over the SHIPPED shim
//! (`amplifier-bundle/tools/gh-merge-guard/gh`). Nothing here reads the real
//! `~/.claude`, `~/.amplihack` or the profile root, and nothing invokes a real
//! `gh`: the bundle is a tempdir, the gate is a synthetic three-line script,
//! and the "real" `gh` behind the guard on `PATH` is a stub that logs its argv.
//!
//! The five facts the Linux `/proc` arm establishes about the immediate parent
//! (`parent_is_gate` in the shim) are:
//!
//! * (a) its pid is still `$PPID`, read before and after the rest;
//! * (b) its argv is `[<argv0>, -p, <physical gate path>, ...]`;
//! * (c) its descriptor 255 is that gate file;
//! * (d) the file it is running is a root-owned `/bin/bash` that this user
//!   cannot write;
//! * (e) its environment carries no exported function and no non-empty
//!   `BASH_ENV`, `ENV`, `SHELLOPTS`, `BASHOPTS`, `PS4`, `LD_PRELOAD`,
//!   `LD_AUDIT`, `LD_LIBRARY_PATH`, `DYLD_INSERT_LIBRARIES` or
//!   `DYLD_LIBRARY_PATH`.
//!
//! On macOS (e) is observed at a different point: the shim's own inherited
//! environment, plus the parent's mapped libraries. `ps -E` withholds the
//! environment of a process whose executable is a SIP-protected platform
//! binary, `/bin/bash` is one, and fact (d) pins the parent to it by inode,
//! so the one process that would have to be read is exactly the case `ps`
//! withholds; `launchctl procinfo` needs root and `vmmap`/`sample` need a
//! debugging entitlement. That difference, the three properties that hold
//! whatever the observation point, and the residual are recorded on #1646,
//! in the PR body and in the reference doc's threat table. The tests below
//! pin the behaviour either way: every injected variable this file plants is
//! still refused, and the DYLD case is refused by the OS before the shim
//! sees it.
//!
//! Two differences from Linux are measured platform facts rather than choices,
//! and each has its own case below: bash 3.2 closes the script descriptor in a
//! forked subshell, so a subshell of the gate cannot satisfy fact (c)
//! ([`a_subshell_of_the_gate_is_refused_because_bash_32_closes_fd_255`]); and
//! macOS `ps` omits an empty `argv[0]` entirely, which makes it
//! indistinguishable from an attack shape
//! ([`an_empty_argv0_is_refused_because_macos_ps_omits_it`]).
//!
//! One load-bearing consequence of the observation point, pinned by
//! [`an_injected_bash_env_or_env_refuses_the_gates_merge`]: the shim's own
//! prologue runs `builtin unset BASH_ENV ENV` before any check, and that
//! removes both names from the environment a child process would see. The
//! macOS arm must therefore snapshot its inherited environment BEFORE that
//! unset, or `BASH_ENV` and `ENV` become invisible to fact (e) and the test
//! fails.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The shim's fixed refusal line for a `gh pr merge`, byte for byte (DP-1:
/// one fact-free line, never argv, so a repeated caller gets no oracle for
/// which fact failed).
const REFUSAL: &str = "REFUSED (amplihack #1551): gh pr merge is only allowed inside \
     autodrive_merge_gate.sh; leave the PR open — auto-drive's merge gate merges after Crusty \
     CLEAN and the merge-ready loop. Do not retry with another gh path, PATH, alias, API client \
     or token.";

/// What the gate does: the one `gh pr merge` whose outcome every case reads.
const MERGE: &str = r#"gh pr merge 1 --squash 2> "$RECORD_DIR/merge.err"
printf '%s' "$?" > "$RECORD_DIR/merge.rc"
"#;

/// Stands in for the real `gh`, behind the guard on `PATH`: logs every argv
/// and succeeds, so a merge that got through shows in its log.
const REAL_GH_STUB: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RECORD_DIR/gh.log\"\nexit 0\n";

fn shipped_shim() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/gh-merge-guard/gh")
        .canonicalize()
        .expect("the shipped gh merge guard")
}

struct Mac {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    records: PathBuf,
    home: PathBuf,
}

/// What one case observed: the launcher's own exit, the recorded `gh pr merge`
/// exit code and stderr, and everything the real `gh` saw.
struct Run {
    out: Output,
    merge_rc: String,
    merge_err: String,
    gh_log: String,
}

impl Run {
    fn saw_merge(&self) -> bool {
        self.gh_log.lines().any(|l| l.starts_with("pr merge"))
    }

    fn describe(&self, what: &str) -> String {
        format!(
            "{what}\n  launcher: {}\n  launcher stdout: {}\n  launcher stderr: {}\n  merge.rc: \
             {:?}\n  merge.err: {:?}\n  gh.log: {:?}",
            self.out.status,
            String::from_utf8_lossy(&self.out.stdout),
            String::from_utf8_lossy(&self.out.stderr),
            self.merge_rc,
            self.merge_err,
            self.gh_log,
        )
    }

    /// The gate's own merge reached the real `gh`, unchanged, and no refusal
    /// was printed.
    fn assert_allowed(&self, what: &str) {
        assert_eq!(self.merge_rc, "0", "{}", self.describe(what));
        assert!(
            self.gh_log.lines().any(|l| l == "pr merge 1 --squash"),
            "{}",
            self.describe(&format!("{what}: the real gh must see the merge unchanged"))
        );
        assert!(
            !self.merge_err.contains("REFUSED"),
            "{}",
            self.describe(&format!("{what}: nothing may be refused"))
        );
    }

    /// Exit 3, the one fixed refusal line, and the real `gh` never saw it.
    fn assert_refused(&self, what: &str) {
        assert_eq!(self.merge_rc, "3", "{}", self.describe(what));
        assert_eq!(
            self.merge_err.trim_end_matches('\n'),
            REFUSAL,
            "{}",
            self.describe(&format!(
                "{what}: the refusal line must be exact and fact-free"
            ))
        );
        assert!(
            !self.saw_merge(),
            "{}",
            self.describe(&format!("{what}: the real gh must never see the merge"))
        );
    }
}

impl Mac {
    fn new() -> Self {
        Self::under("ah")
    }

    /// A bundle whose `AMPLIHACK_HOME` is `<tmp>/<component>`, so a case can
    /// put a space or a backslash in the gate's path.
    fn under(component: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join(component);
        let records = root.join("records");
        let real_gh = root.join("real-gh");
        for d in [&records, &real_gh, &root.join("home")] {
            fs::create_dir_all(d).unwrap();
        }
        let f = Self {
            _tmp: tmp,
            root,
            records,
            home,
        };
        fs::create_dir_all(f.guard_dir()).unwrap();
        set_mode(&f.tools(), 0o755);
        set_mode(&f.guard_dir(), 0o755);
        let shim = shipped_shim();
        crate::support::exec_stub::copy_executable(&shim, &f.guard_dir().join("gh"));
        set_mode(&f.guard_dir().join("gh"), 0o755);
        fs::copy(
            shim.with_file_name("bash-env"),
            f.guard_dir().join("bash-env"),
        )
        .unwrap();
        set_mode(&f.guard_dir().join("bash-env"), 0o644);
        crate::support::exec_stub::write_executable(
            &f.real_gh_dir().join("gh"),
            REAL_GH_STUB,
            0o755,
        );
        f.write_gate(MERGE);
        f
    }

    fn tools(&self) -> PathBuf {
        self.home.join("amplifier-bundle/tools")
    }

    fn guard_dir(&self) -> PathBuf {
        self.tools().join("gh-merge-guard")
    }

    fn gate(&self) -> PathBuf {
        self.tools().join("autodrive_merge_gate.sh")
    }

    fn real_gh_dir(&self) -> PathBuf {
        self.root.join("real-gh")
    }

    /// The synthetic gate: `#!/bin/bash` and `body`. Never a copy of the real
    /// gate, and never carrying the shim's marker.
    fn write_gate(&self, body: &str) {
        crate::support::exec_stub::write_executable(
            &self.gate(),
            format!("#!/bin/bash\n{body}"),
            0o755,
        );
    }

    /// Writes an extra script beside the gate and returns its path.
    fn helper(&self, name: &str, body: &str) -> PathBuf {
        let path = self.tools().join(name);
        crate::support::exec_stub::write_executable(&path, format!("#!/bin/bash\n{body}"), 0o755);
        path
    }

    /// Runs `script` under `/bin/bash -c` in the controlled environment phase
    /// 3 of auto-drive actually uses: no inherited variables at all beyond
    /// `PATH` (guard first, the real `gh` stub behind it), `HOME`,
    /// `AMPLIHACK_HOME`, `RECORD_DIR` and `GATE_PHYS`, plus `extra_env`.
    fn launch(&self, script: &str, extra_env: &[(&str, &str)]) -> Run {
        let path = format!(
            "{}:{}:/usr/bin:/bin:/usr/sbin",
            self.guard_dir().display(),
            self.real_gh_dir().display()
        );
        let mut c = Command::new("/bin/bash");
        c.arg("-c")
            .arg(script)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", std::env::temp_dir())
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_HOME", &self.home)
            .env("GATE_PHYS", gate_phys(&self.gate()));
        for (k, v) in extra_env {
            c.env(k, v);
        }
        let out = c.output().expect("failed to launch the scenario");
        let read = |name: &str| fs::read_to_string(self.records.join(name)).unwrap_or_default();
        Run {
            out,
            merge_rc: read("merge.rc"),
            merge_err: read("merge.err"),
            gh_log: read("gh.log"),
        }
    }

    /// Phase 3's own launch line, verbatim from
    /// `amplifier-bundle/recipes/autodrive-merge-loop.yaml` step-03.
    fn launch_gate_as_phase_3_does(&self) -> Run {
        self.launch(
            r#"( unset -f $(compgen -A function) 2>/dev/null
                 export -n SHELLOPTS BASHOPTS 2>/dev/null
                 unset BASH_ENV ENV PS4
                 exec env -u LD_PRELOAD -u LD_AUDIT -u LD_LIBRARY_PATH /bin/bash -p "$GATE_PHYS" )"#,
            &[],
        )
    }
}

/// The same physical path step-03 computes: `cd -P` the directory, then the
/// basename. On macOS this turns `/var/...` into `/private/var/...`.
fn gate_phys(gate: &Path) -> PathBuf {
    gate.parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join(gate.file_name().unwrap())
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

// ---------------------------------------------------------------------------
// Case 1: the merge the whole feature exists for.
// ---------------------------------------------------------------------------

/// #1646: on macOS the gate's own merge must pass through, exactly as it does
/// on Linux. Before this change the shim had no way to read its parent
/// without `/proc` and refused.
#[test]
fn the_gates_own_merge_is_allowed_on_macos() {
    let f = Mac::new();
    f.launch_gate_as_phase_3_does()
        .assert_allowed("the gate's own merge, launched as phase 3 launches it");
}

/// Facts (a)-(e) gate the merge forms only. Everything else is handed to the
/// real `gh` with `exec`, argv unchanged, whoever the parent is.
#[test]
fn a_non_merge_call_passes_through_from_the_gate_and_from_anywhere_else() {
    let f = Mac::new();
    f.write_gate("gh pr view 1\n");
    let from_gate = f.launch_gate_as_phase_3_does();
    assert!(
        from_gate.gh_log.lines().any(|l| l == "pr view 1"),
        "{}",
        from_gate.describe("gh pr view from the gate must reach the real gh")
    );

    let f = Mac::new();
    let elsewhere = f.launch("gh pr view 2\n", &[]);
    assert!(
        elsewhere.gh_log.lines().any(|l| l == "pr view 2"),
        "{}",
        elsewhere.describe("gh pr view from a plain shell must reach the real gh")
    );
}

// ---------------------------------------------------------------------------
// Case 2: anyone who is not the gate.
// ---------------------------------------------------------------------------

/// The refusal every other case is measured against: a merge from a shell
/// that is not the gate exits 3 with the one fixed line.
#[test]
fn a_merge_from_a_shell_that_is_not_the_gate_is_refused() {
    let f = Mac::new();
    f.launch(MERGE, &[]).assert_refused("a plain bash parent");
}

/// Authorization is not transitive: the gate's grandchild is not the gate.
/// The shim checks its IMMEDIATE parent, which here is the inner bash.
#[test]
fn a_grandchild_of_the_gate_is_refused() {
    let f = Mac::new();
    let inner = f.helper("inner-1646.sh", MERGE);
    f.write_gate(&format!("/bin/bash {}\n", inner.display()));
    f.launch_gate_as_phase_3_does()
        .assert_refused("a grandchild of the gate");
}

/// The one place macOS ends up stricter than Linux, and it is the platform
/// that decides, not this change: **bash 3.2 closes the script descriptor in
/// a forked subshell**. Measured on this host — `lsof -p <subshell> -F fn`
/// lists `cwd`, `txt` and fds 0, 1, 2 and nothing else, where bash 5 on Linux
/// still shows fd 255 — so fact (c) cannot hold for a subshell of the gate
/// here, and `/bin/bash` is the only bash macOS ships.
///
/// Fact (c) is not relaxed to accommodate it. Fd 255 is what pins the file
/// bash actually opened, by inode, to the trusted gate; without it a path
/// that resolved to the gate when bash opened it and to something else when
/// the shim looked would pass. Trading that for a subshell is the wrong way
/// round, so the subshell is refused and the difference is documented.
///
/// It costs the feature nothing, and this test holds that true: the shipped
/// gate's one merge call is a top-level command, not inside `( … )`, which
/// the second half asserts so that a refactor which wrapped it cannot
/// silently stop macOS merging.
#[test]
fn a_subshell_of_the_gate_is_refused_because_bash_32_closes_fd_255() {
    let f = Mac::new();
    f.write_gate(&format!("( {MERGE} )\n"));
    f.launch_gate_as_phase_3_does()
        .assert_refused("a subshell of the gate (bash 3.2 closes its fd 255)");

    let gate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/autodrive_merge_gate.sh");
    let text = fs::read_to_string(&gate).expect("the shipped merge gate");
    let calls: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("gh \"${MERGE_ARGV[@]}\""))
        .collect();
    assert_eq!(
        calls,
        vec!["gh \"${MERGE_ARGV[@]}\""],
        "the gate's merge call must stay a top-level command with no leading \
         whitespace: inside a subshell, or any block that forks, bash 3.2 \
         would drop fd 255 and macOS could never merge ({})",
        gate.display()
    );
}

/// A gate-shaped script that is not the gate beside the shim. The shim trusts
/// only `<its own dir>/../autodrive_merge_gate.sh`, never a copy.
#[test]
fn a_copy_of_the_gate_elsewhere_is_refused() {
    let f = Mac::new();
    let copy = f.root.join("elsewhere/autodrive_merge_gate.sh");
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    crate::support::exec_stub::write_executable(&copy, format!("#!/bin/bash\n{MERGE}"), 0o755);
    f.launch(
        &format!("exec /bin/bash -p {}", gate_phys(&copy).display()),
        &[],
    )
    .assert_refused("a copy of the gate outside the bundle");
}

// ---------------------------------------------------------------------------
// Fact (b): argv. `comm` is argv[0] and is attacker-set, so the remainder
// after subtracting it must be exactly `-p <gate>` (SEC-1, D2).
// ---------------------------------------------------------------------------

/// Verified on this host: `lsof -d 255` names the gate whether or not `-p`
/// was given, so fact (c) does NOT catch a missing `-p`. Only argv does, and
/// without `-p` bash would source `BASH_ENV` and import exported functions —
/// either of which could answer an identity check.
#[test]
fn the_gate_run_without_privileged_mode_is_refused() {
    let f = Mac::new();
    f.launch("exec /bin/bash \"$GATE_PHYS\"", &[])
        .assert_refused("the gate without -p");
}

/// The `exec -a` padding spoof (A3/D2): argv[0] is set to the text a naive
/// scan of the joined `args` would look for, and the real `-p` is dropped.
/// Subtracting `comm` leaves `<gate>`, not `-p <gate>`, so it is refused.
/// A simplification that matched `args` alone would allow this.
#[test]
fn an_argv0_padded_to_look_like_p_gate_is_refused() {
    let f = Mac::new();
    f.launch(
        r#"exec -a "/bin/bash -p $GATE_PHYS" /bin/bash "$GATE_PHYS""#,
        &[],
    )
    .assert_refused("exec -a padding that fakes `-p <gate>` inside argv[0]");
}

/// SEC-1: `set -f` turns off pathname expansion, not pattern matching. An
/// unquoted `${args#$comm}` or an unquoted right-hand side of `[[ == ]]`
/// would let glob metacharacters in argv[0] — which the attacker sets — strip
/// a different amount of text than they occupy, or match text they should
/// not. argv[0] here holds `*`, `?` and `[`; the merge must still be refused.
#[test]
fn glob_metacharacters_in_argv0_cannot_buy_a_merge() {
    let f = Mac::new();
    for argv0 in [
        r#"/bin/bash -p $GATE_PHYS*"#,
        r#"/bin/bash -p ?GATE?"#,
        r#"/bin/bash -p [a-z]*"#,
        r#"*"#,
    ] {
        f.launch(&format!(r#"exec -a "{argv0}" /bin/bash "$GATE_PHYS""#), &[])
            .assert_refused(&format!("argv[0] {argv0:?} with no -p given"));
    }
}

/// The other half of D2, and deliberate on both platforms: argv[0] itself is
/// untrusted. The Linux arm checks argv[1] and argv[2] only, so a junk
/// argv[0] with a real `-p <gate>` is allowed there — and must be allowed
/// here, or macOS would refuse a launcher that Linux accepts. `-p` is in the
/// list on purpose: an argv[0] that repeats the flag still leaves a real
/// `-p <gate>` after the subtraction.
///
/// An EMPTY argv[0] is the one value that is not here; see
/// [`an_empty_argv0_is_refused_because_macos_ps_omits_it`].
#[test]
fn a_junk_argv0_with_a_real_p_gate_is_allowed() {
    let f = Mac::new();
    for argv0 in ["JUNK", "-p", "/usr/bin/true"] {
        let run = f.launch(
            &format!(r#"exec -a "{argv0}" /bin/bash -p "$GATE_PHYS""#),
            &[],
        );
        run.assert_allowed(&format!("argv[0] {argv0:?} with a real -p <gate>"));
        fs::remove_file(f.records.join("gh.log")).ok();
    }
}

/// An empty argv[0] is refused on macOS, where Linux allows it, and it is
/// macOS's `ps` that forces this rather than a choice in the check. Measured
/// on this host: for a parent run as `exec -a "" /bin/bash -p <gate>`, `ps`
/// omits the empty argv[0] entirely and reports `comm=-p` and
/// `args=-p <gate> …` — the same two strings it reports for
/// `exec -a "-p" /bin/bash <gate>`, which is an attack shape with no real
/// `-p` and so must be refused. The two are indistinguishable in the only
/// records macOS offers, so both are refused.
///
/// (`ps` even runs past `argc` into the environment for such a process, which
/// is a second reason not to try to tell them apart.) No launcher uses an
/// empty argv[0]; phase 3 execs `/bin/bash -p "$GATE_PHYS"`, so argv[0] is
/// `/bin/bash`.
#[test]
fn an_empty_argv0_is_refused_because_macos_ps_omits_it() {
    let f = Mac::new();
    f.launch(r#"exec -a "" /bin/bash -p "$GATE_PHYS""#, &[])
        .assert_refused("an empty argv[0], which macOS ps does not report");
}

/// D2/R2, fail closed and documented: a gate path holding whitespace or a
/// backslash cannot be recovered from the space-joined `args` macOS gives, so
/// it is refused rather than guessed at. The cost is a documented constraint
/// on `AMPLIHACK_HOME`, not a weaker check — facts (c) and (d) still prove
/// the file and the interpreter independently.
#[test]
fn a_gate_path_with_whitespace_or_a_backslash_is_refused() {
    for component in ["ah home", "ah\thome", "ah\\home"] {
        let f = Mac::under(component);
        f.launch_gate_as_phase_3_does()
            .assert_refused(&format!("a gate under a path component {component:?}"));
    }
}

// ---------------------------------------------------------------------------
// Fact (c): descriptor 255. Parsed as an `lsof -F fn` RECORD stream (SEC-2).
// ---------------------------------------------------------------------------

/// `bash -c` keeps no descriptor 255, so a gate whose text is replayed
/// through `-c` is refused however right its argv looks. Verified on this
/// host: `lsof -d 255` is empty for `bash -p -c`.
#[test]
fn the_gates_text_replayed_through_bash_c_is_refused() {
    let f = Mac::new();
    f.launch(r#"exec /bin/bash -p -c "$(cat "$GATE_PHYS")""#, &[])
        .assert_refused("the gate's text through bash -p -c");
}

/// SEC-2: `lsof -F fn` is a record stream — a leading `p<pid>` line, then
/// `f<fd>` / `n<name>` pairs. A line-oriented scan that accepted any `n` name
/// matching the gate alongside any `f255` from another record would let a
/// parent satisfy fact (c) with ANY descriptor on the gate file, which
/// `exec 7<gate` arranges trivially. Here the parent holds fd 7 on the gate
/// and no fd 255: refused.
#[test]
fn an_extra_descriptor_on_the_gate_does_not_satisfy_fact_c() {
    let f = Mac::new();
    f.launch(
        r#"exec 7< "$GATE_PHYS"; exec /bin/bash -p -c "$(cat "$GATE_PHYS")""#,
        &[],
    )
    .assert_refused("fd 7 on the gate but no fd 255");
}

// ---------------------------------------------------------------------------
// Fact (e): the parent's environment, observed through the shim's own
// inherited environment on macOS (D1).
// ---------------------------------------------------------------------------

/// An exported shell function reaches the shim as a `BASH_FUNC_*` variable
/// even under `-p` (verified on this host; its value spans several lines, so
/// the scan must anchor the NAME at the start of a line). `-p` stops bash
/// importing it, but its presence means the launcher was not the clean
/// subshell phase 3 builds, so the merge is refused.
#[test]
fn an_exported_function_in_the_environment_refuses_the_gates_merge() {
    let f = Mac::new();
    for name in ["f", "gh", "parent_is_gate", "sys_tool"] {
        f.launch(
            &format!(r#"{name}() {{ :; }}; export -f {name}; exec /bin/bash -p "$GATE_PHYS""#),
            &[],
        )
        .assert_refused(&format!("an exported function {name:?} in the environment"));
    }
}

/// `BASH_ENV` and `ENV` are the two names the shim's own prologue unsets
/// (`builtin unset BASH_ENV ENV`), and that unset also removes them from the
/// environment a child process sees — verified on this host. Fact (e) must
/// therefore be read from a snapshot taken BEFORE the prologue's unset. If
/// the macOS arm shells out to `env` afterwards, these two cases fail while
/// the rest of this test passes: that asymmetry is the whole point of the
/// case.
#[test]
fn an_injected_bash_env_or_env_refuses_the_gates_merge() {
    let f = Mac::new();
    let decoy = f.root.join("decoy.sh");
    fs::write(&decoy, ":\n").unwrap();
    for name in ["BASH_ENV", "ENV"] {
        f.launch(
            &format!(
                r#"exec env {name}={} /bin/bash -p "$GATE_PHYS""#,
                decoy.display()
            ),
            &[],
        )
        .assert_refused(&format!("{name} set at the gate's launch"));
    }
}

/// The rest of (e)'s shell and loader names. `SHELLOPTS` and `BASHOPTS` are
/// rewritten by bash but stay exported and non-empty, so presence — not
/// value — is the test. `LD_*` are not dropped by `-p`.
#[test]
fn injected_shell_or_loader_variables_refuse_the_gates_merge() {
    let f = Mac::new();
    for (name, value) in [
        ("SHELLOPTS", "xtrace"),
        ("BASHOPTS", "cdspell"),
        ("PS4", "+"),
        ("LD_PRELOAD", "/tmp/injected-1646.so"),
        ("LD_AUDIT", "/tmp/injected-1646.so"),
        ("LD_LIBRARY_PATH", "/tmp"),
    ] {
        f.launch(
            &format!(r#"exec env {name}={value} /bin/bash -p "$GATE_PHYS""#),
            &[],
        )
        .assert_refused(&format!("{name}={value} set at the gate's launch"));
    }
}

/// The macOS platform fact, recorded rather than asserted the wrong way:
/// `dyld` strips `DYLD_INSERT_LIBRARIES` and `DYLD_LIBRARY_PATH` for the
/// SIP-restricted `/bin/bash` before the gate even starts (verified on this
/// host: absent from the gate's own environment), and fact (d) pins the
/// parent's mapped executable to `/bin/bash` by inode, so the SIP property is
/// guaranteed, not assumed. The merge is therefore ALLOWED. Asserting a
/// refusal here would encode a mechanism that does not exist and would make
/// the guard refuse any developer who has `DYLD_LIBRARY_PATH` set.
#[test]
fn dyld_variables_are_stripped_by_the_os_so_the_gates_merge_still_works() {
    let f = Mac::new();
    for name in ["DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH"] {
        let run = f.launch(
            &format!(r#"exec env {name}=/tmp/injected-1646.dylib /bin/bash -p "$GATE_PHYS""#),
            &[],
        );
        run.assert_allowed(&format!("{name} set at the gate's launch (dyld strips it)"));
        fs::remove_file(f.records.join("gh.log")).ok();
    }
}

// ---------------------------------------------------------------------------
// Fact (a): liveness. Re-read after (b)-(e), so a parent that exited
// mid-check and whose pid was reused is never judged by a stranger.
// ---------------------------------------------------------------------------

/// A parent that `exec`s away from the gate is no longer the gate. `exec`
/// replaces the process, so the shim's parent here is the launcher, not the
/// gate — argv, fd 255 and the mapped file all belong to someone else.
#[test]
fn a_gate_that_execs_away_is_no_longer_the_parent_the_shim_trusts() {
    let f = Mac::new();
    // The gate hands its own process to a plain bash, which then merges.
    let inner = f.helper("exec-away-1646.sh", MERGE);
    f.write_gate(&format!("exec /bin/bash {}\n", inner.display()));
    f.launch_gate_as_phase_3_does()
        .assert_refused("a gate that exec'd a plain bash before the merge");
}

// ---------------------------------------------------------------------------
// Availability, not authorization: the tools the macOS arm needs.
// ---------------------------------------------------------------------------

/// A5/D14: simulating a missing or SIP-blocked `/usr/sbin/lsof` would need
/// root, or an env-settable tool-root override that would itself be an
/// injection vector into the trusted path — so the missing-tool branch is
/// covered BY CONSTRUCTION (every tool is resolved through the shim's
/// `sys_tool`, which returns failure when the tool is absent, and every
/// helper returns failure on anything but positive establishment). What this
/// test pins instead is that the two tools the arm depends on really are at
/// the absolute paths the launch check names, root-owned, not setuid-writable
/// and not group- or other-writable, so `sys_tool` finds them and the launch
/// check's promise holds on a stock macOS host.
#[test]
fn the_macos_tools_are_where_the_launch_check_says_they_are() {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    for tool in [
        "/bin/ps",
        "/usr/sbin/lsof",
        "/usr/bin/stat",
        "/usr/bin/env",
        "/usr/bin/uname",
    ] {
        let meta = fs::metadata(tool).unwrap_or_else(|e| panic!("{tool}: {e}"));
        assert!(meta.is_file(), "{tool} must be a file");
        assert!(
            meta.permissions().mode() & 0o111 != 0,
            "{tool} must be executable"
        );
        assert_eq!(meta.uid(), 0, "{tool} must be root-owned");
        assert_eq!(
            meta.permissions().mode() & 0o022,
            0,
            "{tool} must not be group- or other-writable"
        );
    }
}

/// The launch check must stop refusing a macOS host. `locate_guard_dir` is
/// the single production call site; this asserts the whole launch path over
/// the real shipped bundle on this Darwin host.
#[test]
fn the_launch_check_no_longer_stops_auto_drive_on_this_host() {
    // The bundle built above is exactly the shape locate_guard_dir wants: an
    // executable `gh`, a `bash-env`, nothing group- or other-writable.
    let f = Mac::new();
    if let Err(detail) = run_an_autodrive_recipe(&f.home) {
        panic!("macOS must pass the #1551 launch check after #1646:\n{detail}");
    }
}

/// `locate_guard_dir` lives in a private module of the binary crate, so the
/// integration test reaches it the way a user does: through the CLI, which
/// fails closed before the runner starts when the host cannot run the guard.
fn run_an_autodrive_recipe(home: &Path) -> Result<(), String> {
    let recipe = home.parent().unwrap().join("autodrive-1646.yaml");
    fs::write(
        &recipe,
        "name: autodrive-1646\ndescription: issue 1646 fixture\nversion: 1.0.0\ncontext: {}\n\
         steps:\n  - id: s\n    type: bash\n    command: \"true\"\n",
    )
    .unwrap();
    let runner = home.parent().unwrap().join("recipe-runner-rs");
    crate::support::exec_stub::write_executable(
        &runner,
        "#!/bin/sh\nprintf '%s\\n' '{\"recipe_name\":\"autodrive-1646\",\"success\":true,\
         \"step_results\":[],\"context\":{}}'\n",
        0o755,
    );
    let out = crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin())
        .arg("recipe")
        .arg("run")
        .arg(&recipe)
        .arg("-w")
        .arg(home.parent().unwrap())
        .current_dir(home.parent().unwrap())
        .env_clear()
        .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", home.parent().unwrap().join("home"))
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_HOME", home)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .output()
        .expect("failed to run amplihack");
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}
