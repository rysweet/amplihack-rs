//! Minimal Rust runner-boundary probe, not a substitute for real-runner coverage.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::{env, fs, path::PathBuf, process::Command};

// Issue #2716: the process primitives the vanishing-task modes need. This
// fixture is compiled with a bare `rustc` and has no crates available, so
// they are declared here rather than taken from `libc`.
unsafe extern "C" {
    fn fork() -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(status: i32) -> !;
}

/// Linux numbers. This fixture only ever runs on the observed platform.
const SIGKILL: i32 = 9;
const ESRCH: i32 = 3;
const ECHILD: i32 = 10;

/// How many times each vanishing-task mode repeats its race. Bounded
/// repetition is the only determinism these modes may use: no sleep, timer,
/// retry or poll appears here or in the tests that drive them. A race that
/// never lands fails its test rather than passing it.
///
/// Raising this is not the remedy when a mode stops landing, and that is
/// worth knowing before spending time on it: what makes these races land is
/// the structure below — the cloning is done by threads that run freely
/// (`clone_underneath`), and `sigkill-child` signals the *previous* child.
/// With either undone the window never opens and no count reopens it. See
/// `docs/reference/run-isolation-vanished-observed-tasks.md`.
const ISSUE_2716_ITERATIONS: usize = 128;

/// Threads of a child's group that clone, so that cloning is still in
/// flight when the group is destroyed underneath them.
const ISSUE_2716_CLONERS: usize = 2;

/// Threads each cloner clones. This is what holds the window open: every
/// one of them costs the observer a registration and two ptrace requests,
/// so while this loop runs there is at any moment a thread of the group at
/// a clone event and a newborn at its initial SIGSTOP.
const ISSUE_2716_THREADS: usize = 32;

/// The result line the CLI reads from its runner. Printed and flushed before
/// a mode starts forking, so no child can inherit it unflushed.
fn announce_success() {
    println!(
        "{{\"recipe_name\":\"issue-2096\",\"success\":true,\"step_results\":[],\"context\":{{}}}}"
    );
    std::io::stdout().flush().unwrap();
}

/// Forks once and returns the child's pid to the parent. The child runs
/// `in_child` and never returns: it leaves through `_exit`, which is
/// `exit_group`, so it runs no destructor and flushes no inherited buffer.
fn fork_child(in_child: impl FnOnce()) -> i32 {
    // SAFETY: this fixture has no threads of its own here, so the child
    // inherits no held lock. The child only clones threads and leaves.
    let child = unsafe { fork() };
    assert_ne!(child, -1, "fork: {}", std::io::Error::last_os_error());
    if child == 0 {
        in_child();
        // SAFETY: leaving the forked child through exit_group, which
        // destroys the thread group including any thread still at its stop.
        unsafe { _exit(0) }
    }
    assert!(child > 0, "implausible fork result {child}");
    child
}

/// Clones threads that do nothing but yield. Each one stops this thread at a
/// clone event and holds the newborn at its initial SIGSTOP until the
/// observer has registered and resumed it.
fn clone_threads() {
    for _ in 0..ISSUE_2716_THREADS {
        // Detached on purpose: the group is destroyed underneath them.
        drop(std::thread::spawn(std::thread::yield_now));
    }
}

/// Starts the cloners and returns while they are still cloning. A thread
/// stopped at a clone event cannot itself destroy its group, because the
/// observer holds it until it has registered and resumed the newborn. So the
/// cloning is handed to threads that run freely, and the group is destroyed
/// underneath them by the caller. Starting the cloners costs one observer
/// round trip each, which is what guarantees they are running by then.
fn clone_underneath() {
    for _ in 0..ISSUE_2716_CLONERS {
        drop(std::thread::spawn(clone_threads));
    }
}

/// Kills one child this fixture forked, by number, and reaps it. `child` is
/// always a positive pid from [`fork_child`]: never 0, never negative, never
/// a name or a pattern, so this can reach no process group and nothing this
/// fixture did not create. SIGKILL needs no handler and reaches a tracee even
/// in ptrace-stop, which is the point of the mode that calls this.
///
/// `ESRCH` is accepted and nothing else is: the child often leaves through
/// `exit_group` on its own and the observer reaps it before we get here, so
/// "no such process" is an ordinary answer. Any other errno means the signal
/// could never be delivered, and this mode would then quietly become a
/// duplicate of `exit-group-thread` while still passing — the test's only
/// non-vacuity assertion is already satisfied by the `exit_group` races the
/// child makes by itself. Failing loudly here is what keeps the mode honest.
fn kill_child(child: i32) {
    assert!(child > 0, "refusing to signal {child}");
    // SAFETY: a positive pid forked and recorded by this fixture.
    if unsafe { kill(child, SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        assert_eq!(
            error.raw_os_error(),
            Some(ESRCH),
            "kill fixture child {child}: {error}"
        );
    }
    wait_for(child);
}

/// Waits for one pid this fixture created, by number. Never a process group,
/// never -1, never a name or a pattern. The observer is this child's tracer
/// and reaps it first, so ECHILD is an ordinary answer here.
fn wait_for(child: i32) {
    let mut status = 0;
    loop {
        // SAFETY: status is writable storage; child is a pid forked above.
        if unsafe { waitpid(child, &mut status, 0) } >= 0 {
            return;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ECHILD) {
            return;
        }
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::Interrupted,
            "wait for fixture child {child}: {error}"
        );
    }
}

fn git(args: &[&str], clean_environment: bool) -> std::process::Output {
    let mut command = Command::new(env::var_os("ISSUE_2096_GIT").unwrap());
    if clean_environment {
        command.env_clear();
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "AMPLIHACK_HOME",
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
        ] {
            command.env(key, env::var_os(key).expect("isolated fixture environment"));
        }
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
    }
    let output = command.args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    output
}

fn main() {
    let arguments: Vec<_> = env::args_os().collect();
    // The launch-time capability probe (runner_validation::probe).
    if arguments.get(1).is_some_and(|a| a == "--capabilities") {
        println!(
            r#"{{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}}"#
        );
        return;
    }
    if std::path::Path::new(&arguments[0]).file_name() == Some(std::ffi::OsStr::new("gh")) {
        let expected: Vec<_> = [
            "pr",
            "view",
            "2290",
            "--json",
            "headRefName",
            "--jq",
            ".headRefName",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect();
        assert_eq!(
            &arguments[1..],
            expected,
            "unexpected network request in the local hook replay"
        );
        println!("hook-replay");
        return;
    }
    if let Some(pair) = arguments.windows(2).find(|pair| pair[0] == "--step-root") {
        let root = PathBuf::from(&pair[1]);
        let home = root.join("home");
        // This fixture is a fresh, single-threaded executable. Restoring the
        // disposable paths through argv also tests a runner that filters env.
        for (key, value) in [
            ("HOME", home.clone()),
            ("XDG_CONFIG_HOME", home.join("config")),
            ("CLAUDE_CONFIG_DIR", home.join(".claude")),
            ("CODEX_HOME", home.join(".codex")),
            ("AMPLIHACK_HOME", home.join(".amplihack")),
            ("AMPLIHACK_CLAUDE_PROFILE_ROOT", home.join("profiles")),
            ("ISSUE_2096_OBSERVATIONS", root.join("real-runner")),
        ] {
            unsafe { env::set_var(key, value) };
        }
        unsafe {
            env::set_var(
                "ISSUE_2096_GIT",
                fs::read_to_string(root.join("git-path")).unwrap(),
            )
        };
        let mode = arguments
            .windows(2)
            .find(|pair| pair[0] == "--mode")
            .expect("real runner fixture mode");
        unsafe { env::set_var("ISSUE_2096_MODE", &mode[1]) };
    }
    if let Some(marker) = env::var_os("ISSUE_2096_PREPARATION_MARKER")
        && (arguments[0].to_string_lossy().contains("post-checkout")
            || arguments.iter().any(|arg| arg == "--filter"))
    {
        fs::write(marker, b"untrusted preparation executed\n").unwrap();
        std::process::exit(17);
    }
    if env::var_os("ISSUE_2096_DRIVER_ROLE").as_deref() == Some(std::ffi::OsStr::new("hook")) {
        fs::write("hook-output", b"hook descendant\n").unwrap();
        git(&["add", "--", "hook-output"], true);
        return;
    }
    run();
}

fn run() {
    let args: Vec<_> = env::args_os().collect();
    let cwd = args
        .windows(2)
        .find(|a| a[0] == "-C")
        .map(|a| PathBuf::from(&a[1]))
        .unwrap_or_else(|| env::current_dir().unwrap());
    env::set_current_dir(&cwd).unwrap();
    let out = PathBuf::from(env::var_os("ISSUE_2096_OBSERVATIONS").unwrap());
    fs::create_dir_all(&out).unwrap();
    if args
        .get(1)
        .is_some_and(|arg| !arg.to_string_lossy().starts_with("--"))
    {
        let recipe = PathBuf::from(&args[1]);
        fs::write(
            out.join("recipe-path"),
            recipe.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        fs::write(
            out.join("recipe-bytes"),
            fs::read(recipe).expect("required invocation recipe input"),
        )
        .unwrap();
    }
    for pair in args.windows(2) {
        if (pair[0] == "--set" || pair[0] == "-c")
            && let Some(path) = pair[1].to_str().and_then(|s| s.strip_prefix("repo_path="))
        {
            fs::write(out.join("repo_path"), path).unwrap();
        }
    }
    fs::write(
        out.join("cwd"),
        fs::canonicalize(".")
            .unwrap()
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    for (name, args) in [
        ("head", vec!["rev-parse", "HEAD"]),
        ("branch", vec!["symbolic-ref", "-q", "HEAD"]),
        ("staged", vec!["show", ":tracked"]),
        (
            "index",
            vec!["rev-parse", "--path-format=absolute", "--git-path", "index"],
        ),
        (
            "common",
            vec!["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ),
        (
            "objects",
            vec![
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
            ],
        ),
        ("entries", vec!["ls-files", "--stage", "-z"]),
    ] {
        let result = Command::new(env::var_os("ISSUE_2096_GIT").unwrap())
            .args(&args)
            .output()
            .unwrap();
        assert!(
            result.status.success() || (name == "branch" && result.status.code() == Some(1)),
            "Git observation {name} {args:?} failed with {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        );
        fs::write(out.join(name), result.stdout).unwrap();
    }
    for name in ["tracked", "untracked input", "binary", "ignored-input"] {
        match fs::read(name) {
            Ok(bytes) => fs::write(out.join(name), bytes).unwrap(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && name != "tracked" => {}
            Err(error) => panic!("read fixture input {name}: {error}"),
        }
    }
    if let Some(name) = env::var_os("ISSUE_2096_INPUT") {
        // Absence is itself an observation, other I/O errors are fixture bugs.
        match fs::read(&name) {
            Ok(bytes) => fs::write(out.join("selected-input"), bytes).unwrap(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("read selected input {name:?}: {error}"),
        }
    }
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
    ] {
        if let Some(value) = env::var_os(name) {
            fs::write(out.join(name), value.as_encoded_bytes()).unwrap();
        }
    }
    if let Some(socket) = env::var_os("ISSUE_2096_RENDEZVOUS") {
        let mut stream = UnixStream::connect(socket).unwrap();
        stream.write_all(b"ready\n").unwrap();
        let mut message = String::new();
        assert_ne!(
            BufReader::new(stream).read_line(&mut message).unwrap(),
            0,
            "parent abandoned runner"
        );
        assert_eq!(message, "go\n");
    }
    match env::var("ISSUE_2096_MODE").as_deref().unwrap_or("observe") {
        "observe" => {}
        "nested" => {
            let before = git(&["rev-parse", "HEAD"], false).stdout;
            let nested = Command::new(env::var_os("ISSUE_2096_AMPLIHACK").unwrap())
                .args(["recipe", "run"])
                .arg(env::var_os("ISSUE_2096_RECIPE").unwrap())
                .args(["-w", "."])
                .env("RECIPE_RUNNER_RS_PATH", env::current_exe().unwrap())
                .env("ISSUE_2096_OBSERVATIONS", out.join("nested"))
                .env("ISSUE_2096_MODE", "observe")
                .env_remove("ISSUE_2096_RENDEZVOUS")
                .output()
                .unwrap();
            assert!(
                nested.status.success(),
                "independent nested invocation: {nested:?}"
            );
            assert_eq!(git(&["rev-parse", "HEAD"], false).stdout, before);
            fs::write(out.join("outer-still-active"), b"yes").unwrap();
        }
        "untraced-add" => {
            fs::write("untraced-output", b"absolute reduced-env git\n").unwrap();
            git(&["add", "--", "untraced-output"], true);
            fs::write(out.join("git-succeeded"), b"yes").unwrap();
        }
        "hook-commit" => {
            let hooks = out.join("hooks");
            fs::create_dir(&hooks).unwrap();
            fs::copy(env::current_exe().unwrap(), hooks.join("pre-commit")).unwrap();
            let hooks_arg = format!("core.hooksPath={}", hooks.display());
            let result = Command::new(env::var_os("ISSUE_2096_GIT").unwrap())
                .env("ISSUE_2096_DRIVER_ROLE", "hook")
                .args([
                    "-c",
                    &hooks_arg,
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "hook coverage",
                ])
                .output()
                .unwrap();
            assert!(result.status.success(), "hook commit: {result:?}");
            fs::write(
                out.join("hook-committed"),
                git(&["show", "HEAD:hook-output"], false).stdout,
            )
            .unwrap();
        }
        "publication-failure" => {
            git(&["add", "--", "tracked"], false);
            let run_dir = PathBuf::from(env::var_os("AMPLIHACK_RUN_DIR").unwrap());
            fs::create_dir(run_dir.join("git-commands.jsonl")).unwrap();
            fs::write(out.join("publication-blocked"), b"yes").unwrap();
        }
        "runtime-replacement" => {
            let runtime=PathBuf::from(env::var_os("AMPLIHACK_WORKFLOW_RUNTIME_DIR").unwrap());
            fs::rename(&runtime,out.join("retained-runtime")).unwrap();
            fs::create_dir(&runtime).unwrap();
            fs::write(runtime.join("unowned-sentinel"),b"keep\n").unwrap();
            fs::write(out.join("replacement-runtime"),runtime.as_os_str().as_encoded_bytes()).unwrap();
        }
        "collection-loss" => {
            git(&["add", "--", "tracked"], false);
            let run_dir = PathBuf::from(env::var_os("AMPLIHACK_RUN_DIR").unwrap());
            let trace = run_dir.join("git-trace2");
            // Fault-inject the currently implemented collector. This is not
            // a claim that Trace2 is the required observer backend.
            fs::rename(&trace, out.join("retained-trace")).unwrap();
            fs::write(&trace, b"collector destination lost").unwrap();
            fs::write(out.join("collection-lost"), b"yes").unwrap();
        }
        // Issue #2716: a thread group whose leader calls exit_group while a
        // freshly cloned thread of that group is still at its initial ptrace
        // stop, and while another thread of the group is stopped at its own
        // clone event. The observer's resume of either then fails with
        // ESRCH, and the newborn's /proc entry may already be gone.
        //
        // The concurrency is what makes this land. A thread stopped at a
        // clone event cannot itself destroy the group: the observer holds it
        // until it has registered and resumed the newborn. So the group's
        // leader hands the cloning to threads that run freely and then calls
        // exit_group underneath them. Nothing here reaches an exec stop.
        "exit-group-thread" => {
            announce_success();
            for _ in 0..ISSUE_2716_ITERATIONS {
                // The closure returns through `_exit`, which is exit_group,
                // while the cloners it started are still cloning.
                let child = fork_child(clone_underneath);
                wait_for(child);
            }
            return;
        }
        // Issue #2716: a task killed with SIGKILL between its stop and the
        // continue. A SIGKILLed tracee dies even in ptrace-stop, so the
        // resume it was waiting for fails with ESRCH.
        //
        // Each child clones threads, so while it runs there is at any moment
        // a thread of its group at a clone event and a newborn at its initial
        // SIGSTOP. What the kill must not do is arrive before any of that
        // exists: a child signalled in the same breath as its fork dies
        // before it has cloned anything, and there is then no held stop to
        // interrupt. So the signal goes to the previous child, which has been
        // cloning throughout the fork of this one. Only ever one pid this
        // fixture forked and recorded, by number: never a group, a negative
        // pid, a name or a pattern, and never more than two children alive.
        // No child execs, so this reaches no exec stop either.
        "sigkill-child" => {
            announce_success();
            // `Option`, never a sentinel: `kill(0, ...)` signals the whole
            // process group, which here is the test harness. Only a pid this
            // fixture forked and recorded can reach `kill_child`.
            let mut cloning = None;
            for _ in 0..ISSUE_2716_ITERATIONS {
                let next = fork_child(clone_underneath);
                if let Some(previous) = cloning {
                    kill_child(previous);
                }
                cloning = Some(next);
            }
            if let Some(last) = cloning {
                kill_child(last);
            }
            return;
        }
        other => panic!("unknown fixture mode {other}"),
    }
    if env::var_os("ISSUE_2096_STAGE").is_some() {
        fs::write("run-output", b"private output\n").unwrap();
        // Absolute invocation deliberately defeats PATH-only Git wrappers.
        let status = Command::new(env::var_os("ISSUE_2096_GIT").unwrap())
            .args(["add", "--", "run-output"])
            .status()
            .unwrap();
        assert!(status.success());
    }
    println!(
        "{{\"recipe_name\":\"issue-2096\",\"success\":true,\"step_results\":[],\"context\":{{}}}}"
    );
}
