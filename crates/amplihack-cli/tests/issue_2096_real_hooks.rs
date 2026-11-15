//! Candidate-1 replay with the actual project pre-commit configuration.
//! This is a present-behavior reproduction, not historical attribution.
use super::*;
use std::io::Write;
use std::os::unix::{
    ffi::OsStrExt,
    fs::{DirBuilderExt, OpenOptionsExt},
    process::CommandExt,
};

fn fixture_command(f: &Fixture, program: &str, directory: &Path) -> Command {
    let toolchain = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("resolve installed test toolchain");
    succeeds(&toolchain);
    let toolchain = PathBuf::from(String::from_utf8(toolchain.stdout).unwrap().trim()).join("bin");
    let mut command = Command::new(program);
    f.environment(&mut command);
    for key in [
        "RECIPE_RUNNER_QA_BINARY",
        "RECIPE_RUNNER_QA_RECEIPT",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "CARGO_HTTP_CAINFO",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .current_dir(directory)
        .env(
            "PATH",
            std::env::join_paths(
                [f.root.join("hook-bin"), toolchain]
                    .into_iter()
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap(),
        )
        .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("PRE_COMMIT_HOME", f.root.join("pre-commit-cache"))
        .env("CARGO_HOME", f.root.join("cargo-home"))
        .env("XDG_CACHE_HOME", f.home.join("cache"))
        .env("AMPLIHACK_WORK_ROOT", f.root.join("work"))
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    // SAFETY: umask is async-signal-safe and touches only this exec child's
    // file creation policy; no Rust runtime operation runs in the closure.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o077);
            Ok(())
        });
    }
    command
}

fn evidence_write(path: &Path, contents: &[u8]) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(contents)
        .unwrap();
}

fn checked_capture(f: &Fixture, mut command: Command, evidence: &Path, label: &str) {
    let output = command.output().unwrap();
    evidence_write(&evidence.join(format!("{label}.stdout")), &output.stdout);
    evidence_write(&evidence.join(format!("{label}.stderr")), &output.stderr);
    evidence_write(
        &evidence.join(format!("{label}.status")),
        output.status.to_string().as_bytes(),
    );
    // Snapshot before asserting so a failing commit/push still retains
    // caller evidence, rather than stopping before the after-state exists.
    evidence_write(
        &evidence.join(format!("{label}.caller.index")),
        &fs::read(f.repo.join(".git/index")).unwrap(),
    );
    evidence_write(
        &evidence.join(format!("{label}.caller.head")),
        &f.git(&["rev-parse", "HEAD"]),
    );
    succeeds(&output);
}

#[test]
#[ignore = "full project hooks build and test the workspace; run explicitly with --ignored"]
fn candidate_one_real_hooks_and_local_push_preserve_the_linked_worktree_callers_state() {
    let mut f = Fixture::new();
    f._temp.disable_cleanup(true);
    let evidence = f.root.join("hook-evidence");
    fs::DirBuilder::new().mode(0o700).create(&evidence).unwrap();
    // Retain diagnostics on an assertion failure, with an explicit receipt.
    let retained = f._temp.path().to_path_buf();
    eprintln!("candidate-1 fixture and evidence: {}", retained.display());
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let paths = f.git_in(
        &source,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    );
    for bytes in paths
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
    {
        let relative = Path::new(std::ffi::OsStr::from_bytes(bytes));
        let from = source.join(relative);
        match fs::symlink_metadata(&from) {
            Ok(metadata) => {
                let to = f.repo.join(relative);
                fs::create_dir_all(to.parent().unwrap()).unwrap();
                if metadata.file_type().is_symlink() {
                    std::os::unix::fs::symlink(fs::read_link(from).unwrap(), to).unwrap();
                } else if metadata.is_file() {
                    fs::copy(from, to).unwrap();
                } else {
                    panic!("unsupported source fixture input {}", relative.display());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("copy {}: {error}", from.display()),
        }
    }
    f.git(&["add", "-A"]);
    // The real repository tracks some ignored paths, including Cargo.lock.
    // Recreate that baseline explicitly: add -A alone would omit them in
    // this new fixture and make the real --locked hooks fail spuriously.
    let tracked = f.git_in(&source, &["ls-files", "--cached", "-z"]);
    let mut add = amplihack_git::command_in(&f.repo);
    f.environment(&mut add);
    let mut child = add
        .args([
            "add",
            "--force",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&tracked).unwrap();
    succeeds(&child.wait_with_output().unwrap());
    f.git(&["commit", "-qm", "complete project hook fixture"]);
    let remote = f.root.join("remote.git");
    f.git(&["init", "--bare", "-q", remote.to_str().unwrap()]);
    f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    let private = f.root.join("linked");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "hook-replay",
        private.to_str().unwrap(),
    ]);
    fs::create_dir(f.root.join("hook-bin")).unwrap();
    std::os::unix::fs::symlink(
        super::contract_support::driver(),
        f.root.join("hook-bin/gh"),
    )
    .unwrap();
    let mut install = fixture_command(&f, "pre-commit", &private);
    install.args([
        "install",
        "--install-hooks",
        "-t",
        "pre-commit",
        "-t",
        "pre-push",
    ]);
    checked_capture(&f, install, &evidence, "install");

    fs::create_dir_all(f.repo.join("private")).unwrap();
    fs::write(
        f.repo.join("private/system-prompt-append.md"),
        b"caller-only input\n",
    )
    .unwrap();
    fs::write(
        private.join("private-staged.rs"),
        b"// staged fixture change\n",
    )
    .unwrap();
    f.git_in(&private, &["add", "private-staged.rs"]);
    fs::write(private.join("tracked"), b"unstaged private change\n").unwrap();
    let before = f.snapshot();
    evidence_write(&evidence.join("caller.index.before"), &before.0);
    evidence_write(&evidence.join("caller.head.before"), &before.1);
    let trace = evidence.join("trace2");
    fs::create_dir(&trace).unwrap();
    let mut target = trace.into_os_string();
    target.push("/");
    let mut guard = fixture_command(&f, "bash", &private);
    guard
        .arg(private.join("amplifier-bundle/tools/autodrive_worktree_guard.sh"))
        .args(["commit-dir", "2290"]);
    checked_capture(&f, guard, &evidence, "commit-dir");
    let mut hygiene = fixture_command(
        &f,
        crate::support::amplihack_bin::amplihack_bin()
            .to_str()
            .unwrap(),
        &private,
    );
    hygiene.args(["hygiene", "artifact-guard", "--repo", "."]);
    checked_capture(&f, hygiene, &evidence, "hygiene");
    let mut add = fixture_command(&f, "git", &private);
    amplihack_git::scrub(&mut add);
    add.env("GIT_TRACE2_EVENT", &target).args(["add", "-A"]);
    checked_capture(&f, add, &evidence, "stage");
    // Keep both staged and unstaged versions so pre-commit takes its actual
    // stash-and-restore path after the round's git add -A.
    fs::write(private.join("tracked"), b"unstaged after staging\n").unwrap();
    for (label, arguments) in [
        (
            "commit",
            vec!["commit", "-m", "candidate-1 real-hook replay"],
        ),
        ("push", vec!["push", "-u", "origin", "hook-replay"]),
    ] {
        let mut command = fixture_command(&f, "git", &private);
        amplihack_git::scrub(&mut command);
        command
            .env("GIT_TRACE2_EVENT", &target)
            .env(
                "GIT_TRACE2_ENV_VARS",
                "GIT_DIR,GIT_INDEX_FILE,GIT_WORK_TREE,PWD",
            )
            .args(arguments);
        checked_capture(&f, command, &evidence, label);
    }
    let after = f.snapshot();
    evidence_write(&evidence.join("caller.index.after"), &after.0);
    evidence_write(&evidence.join("caller.head.after"), &after.1);
    assert_eq!(
        after, before,
        "candidate-1 replay changed caller state; inspect retained hook output and routing evidence"
    );
    assert_eq!(
        fs::read(private.join("tracked")).unwrap(),
        b"unstaged after staging\n"
    );
    assert!(
        f.git_in(&remote, &["show-ref", "--verify", "refs/heads/hook-replay"])
            .len()
            > 40
    );
    // Trace2 is supplemental diagnostic evidence only. Descendant coverage
    // and any historical writer attribution require separate evidence.
    evidence_write(
        &evidence.join("disposition.txt"),
        b"present replay completed; historical attribution unproven\n",
    );
}
