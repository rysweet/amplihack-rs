//! Minimal Rust runner-boundary probe, not a substitute for real-runner coverage.
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let args: Vec<_> = env::args_os().collect();
    let cwd = args
        .windows(2)
        .find(|a| a[0] == "-C")
        .map(|a| PathBuf::from(&a[1]))
        .expect("runner must receive -C");
    env::set_current_dir(&cwd).unwrap();
    let out = PathBuf::from(env::var_os("ISSUE_2096_OBSERVATIONS").unwrap());
    fs::create_dir_all(&out).unwrap();
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
    for name in ["tracked", "untracked input", "binary"] {
        match fs::read(name) {
            Ok(bytes) => fs::write(out.join(name), bytes).unwrap(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && name != "tracked" => {}
            Err(error) => panic!("read fixture input {name}: {error}"),
        }
    }
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
    ] {
        if let Some(value) = env::var_os(name) {
            fs::write(out.join(name), value.as_encoded_bytes()).unwrap();
        }
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
