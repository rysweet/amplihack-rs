//! Minimal Rust runner-boundary probe, not a substitute for real-runner coverage.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::{env, fs, path::PathBuf, process::Command};

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
