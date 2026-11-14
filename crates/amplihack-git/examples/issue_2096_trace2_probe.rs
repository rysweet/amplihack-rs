//! Probe Trace2 attribution in disposable repositories; this is not a recorder.
//!
//! Run with `cargo run -p amplihack-git --example issue_2096_trace2_probe -- DIR`.
//! DIR must not exist. Evidence is retained there with owner-only permissions.
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output};

fn checked(command: &mut Command, evidence: &Path) -> io::Result<Output> {
    let output = command.output()?;
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(evidence.join("commands.txt"))?;
    writeln!(log, "{command:?}\nstatus: {}", output.status)?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;
    log.sync_all()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "probe command failed: {}; see commands.txt",
            output.status
        )));
    }
    Ok(output)
}

fn git(root: &Path, cwd: &Path) -> Command {
    let mut command = amplihack_git::command_in(cwd);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .env("HOME", root.join("home"))
        .env("TMPDIR", root)
        .env("XDG_CONFIG_HOME", root.join("home/config"))
        .env("CLAUDE_CONFIG_DIR", root.join("home/.claude"))
        .env("CODEX_HOME", root.join("home/.codex"))
        .env("AMPLIHACK_HOME", root.join("home/.amplihack"))
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", root.join("home/profiles"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

/// Exercise the OS observer separately. Availability is not a coverage proof:
/// in particular, macOS may install dtruss while denying DTrace access.
fn probe_observer(root: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let observer = Some(("/usr/bin/dtruss", vec!["-f", "/usr/bin/true"]));
    #[cfg(target_os = "linux")]
    let observer = Some((
        "/usr/bin/strace",
        vec!["-f", "-e", "trace=process,chdir,fchdir", "/usr/bin/true"],
    ));
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let observer: Option<(&str, Vec<&str>)> = None;
    let Some((executable, args)) = observer else {
        fs::write(root.join("observer.status"), "unsupported platform\n")?;
        return Ok(());
    };
    let outcome = Command::new(executable)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("TMPDIR", root)
        .current_dir(root)
        .output();
    match outcome {
        Ok(output) => {
            fs::write(root.join("observer.stdout"), output.stdout)?;
            fs::write(root.join("observer.stderr"), output.stderr)?;
            fs::write(
                root.join("observer.status"),
                format!("{executable}: {}\ncoverage_verified=false\n", output.status),
            )?;
        }
        Err(error) => fs::write(
            root.join("observer.status"),
            format!("{executable}: {error}\ncoverage_verified=false\n"),
        )?,
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = std::env::args_os().nth(1).ok_or("missing evidence DIR")?;
    let destination = Path::new(&destination);
    // Refuse reuse rather than following an existing directory or symlink.
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(destination)?;
    }
    #[cfg(not(unix))]
    return Err("probe requires Unix owner-only evidence storage".into());
    let root = fs::canonicalize(destination)?;
    fs::create_dir(root.join("home"))?;
    probe_observer(&root)?;
    let caller = root.join("caller");
    let checkout = root.join("checkout");
    fs::create_dir(&caller)?;
    checked(git(&root, &caller).args(["--version"]), &root)?;
    checked(git(&root, &caller).args(["init", "-q"]), &root)?;
    fs::write(caller.join("tracked"), b"input\n")?;
    checked(git(&root, &caller).args(["add", "tracked"]), &root)?;
    checked(
        git(&root, &caller).args([
            "-c",
            "user.name=Probe",
            "-c",
            "user.email=probe@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ]),
        &root,
    )?;
    checked(
        git(&root, &caller)
            .args(["worktree", "add", "--detach"])
            .arg(&checkout)
            .arg("HEAD"),
        &root,
    )?;
    let caller_index = caller.join(".git/index");
    let before = fs::read(&caller_index)?;
    fs::write(root.join("caller-index.before"), &before)?;
    fs::write(caller.join("untracked-main-input"), b"caller only\n")?;
    fs::write(checkout.join("private-output"), b"private\n")?;
    let index = checked(
        git(&root, &checkout).args(["rev-parse", "--path-format=absolute", "--git-path", "index"]),
        &root,
    )?;
    fs::write(root.join("expected-index.txt"), &index.stdout)?;
    // An absolute invocation with -C tests evidence beyond argv[0] and launch cwd.
    let executable = ["/usr/bin/git", "/opt/homebrew/bin/git"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .ok_or("no absolute Git executable available")?;
    let mut command = git(&root, &root);
    let environment: Vec<_> = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(|v| v.to_owned())))
        .collect();
    command = Command::new(executable);
    command.current_dir(&root).env_clear();
    for (key, value) in environment {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    command
        .env("GIT_TRACE2_EVENT", root.join("trace2.jsonl"))
        .env(
            "GIT_TRACE2_ENV_VARS",
            "GIT_DIR,GIT_WORK_TREE,GIT_INDEX_FILE",
        )
        .arg("-C")
        .arg(&checkout)
        .args(["add", "--", "private-output"]);
    checked(&mut command, &root)?;
    // Identical argv and relative routing from two distinct launch cwds.
    // Git may normalize its own cwd; inspect rather than assume index semantics.
    for name in ["one", "two"] {
        let cwd = checkout.join(name);
        fs::create_dir(&cwd)?;
        fs::write(cwd.join("local-input"), b"descendant input\n")?;
        command
            .current_dir(&cwd)
            .env("GIT_INDEX_FILE", "alternate.index")
            .env(
                "GIT_TRACE2_EVENT",
                root.join(format!("trace2-{name}.jsonl")),
            );
        // Keep the same sanitized profile environment, but remove the earlier -C.
        let mut descendant = Command::new(executable);
        descendant.current_dir(&cwd).env_clear();
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                descendant.env(key, value);
            }
        }
        descendant.args(["add", "--", "local-input"]);
        checked(&mut descendant, &root)?;
        let expected = checked(
            git(&root, &cwd)
                .env("GIT_INDEX_FILE", "alternate.index")
                .args(["rev-parse", "--path-format=absolute", "--git-path", "index"]),
            &root,
        )?;
        fs::write(
            root.join(format!("expected-index-{name}.txt")),
            expected.stdout,
        )?;
    }
    // A real Git hook launches a shell descendant, which launches absolute Git
    // from a changed cwd. This is controlled diagnostic code, NOT the project's
    // production hook set or a trusted reviewer launch.
    let hooks = root.join("hooks");
    fs::create_dir(&hooks)?;
    let hook = hooks.join("pre-commit");
    fs::write(
        &hook,
        b"#!/bin/sh\nset -eu\nunset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX GIT_COMMON_DIR\ncd one\n\"$PROBE_GIT\" rev-parse --show-toplevel > \"$PROBE_HOOK_MARKER\"\nunset GIT_TRACE2_EVENT\n\"$PROBE_GIT\" rev-parse --git-dir > \"$PROBE_BYPASS_MARKER\"\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700))?;
    }
    let mut shell = Command::new("/bin/sh");
    shell.current_dir(&checkout).env_clear();
    for (key, value) in git(&root, &checkout).get_envs() {
        if let Some(value) = value {
            shell.env(key, value);
        }
    }
    shell
        .env("PROBE_GIT", executable)
        .env("PROBE_HOOK_MARKER", root.join("hook.marker"))
        .env("PROBE_BYPASS_MARKER", root.join("hook-bypass.marker"))
        .env("GIT_TRACE2_EVENT", root.join("trace2-hook.jsonl"))
        .args(["-c", "exec \"$@\"", "probe", executable, "-c"])
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args([
            "-c",
            "user.name=Probe",
            "-c",
            "user.email=probe@example.invalid",
            "commit",
            "-qm",
            "private linked checkout commit",
        ]);
    checked(&mut shell, &root)?;
    if fs::read(root.join("hook.marker"))? != format!("{}\n", checkout.display()).as_bytes() {
        return Err("hook descendant did not resolve the private checkout".into());
    }
    let hook_trace = fs::read_to_string(root.join("trace2-hook.jsonl"))?;
    let bypass_ran = !fs::read(root.join("hook-bypass.marker"))?.is_empty();
    let bypass_recorded = hook_trace.contains("\"--git-dir\"");
    fs::write(
        root.join("hook-bypass.result"),
        format!("command_executed={bypass_ran}\ncommand_recorded={bypass_recorded}\n"),
    )?;
    // Trace2's native initialization policy must not be mistaken for fail-closed
    // recording. Retain the command's real status and warning for inspection.
    let blocked = root.join("not-a-directory");
    fs::write(&blocked, b"occupied\n")?;
    fs::write(checkout.join("private-output"), b"changed without trace\n")?;
    let private_index = std::path::PathBuf::from(std::str::from_utf8(&index.stdout)?.trim());
    let private_before = fs::read(&private_index)?;
    fs::write(
        root.join("private-index.before-recording-failure"),
        &private_before,
    )?;
    let mut failed_recording = git(&root, &checkout);
    failed_recording
        .env("GIT_TRACE2_EVENT", blocked.join("events.jsonl"))
        .args(["add", "--", "private-output"]);
    let result = failed_recording.output()?;
    fs::write(root.join("recording-failure.stderr"), &result.stderr)?;
    fs::write(
        root.join("recording-failure.status"),
        format!("{}\n", result.status),
    )?;
    let private_after = fs::read(private_index)?;
    fs::write(
        root.join("private-index.after-recording-failure"),
        &private_after,
    )?;
    fs::write(
        root.join("recording-failure.index-changed"),
        format!("{}\n", private_before != private_after),
    )?;
    let after = fs::read(caller_index)?;
    fs::write(root.join("caller-index.after"), &after)?;
    if before != after {
        return Err("probe changed the caller index".into());
    }
    println!("Evidence: {}", root.display());
    println!("Inspect trace2.jsonl against commands.txt and expected-index.txt.");
    println!("Parent command evidence is NOT descendant attribution.");
    Ok(())
}
