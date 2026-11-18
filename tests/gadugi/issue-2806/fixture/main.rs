//! Test-only executable. `run-all` is exercised in CI, never on factory hosts.
#![cfg(unix)]
// Both the executable and cli_integration compile the shared fixture modules.
#![allow(dead_code)]
mod agent;
mod driver;
mod environment;
mod evidence;
mod hosting;
mod lifecycle;
mod responses;

use anyhow::{Result, bail, ensure};
use environment::IsolatedCase;
use std::io::{Read, Write};

fn main() {
    if let Err(error) = execute() {
        if let Ok(root) = std::env::var("AMPLIHACK_2806_ROOT") {
            let path = std::path::Path::new(&root)
                .join("evidence")
                .join(format!("fixture-error-{}.txt", uuid::Uuid::new_v4()));
            if let Err(record_error) = std::fs::write(&path, format!("{error:#}\n")) {
                eprintln!(
                    "cannot retain fixture error at {}: {record_error}",
                    path.display()
                );
            }
        }
        eprintln!("issue-2806 harness error: {error:#}");
        std::process::exit(1);
    }
}
fn execute() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let name = std::path::Path::new(&args[0])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if matches!(
        name,
        "gh" | "claude" | "codex" | "git-remote-issue2806" | "model-launcher"
    ) {
        return fixture(name, &args[1..]);
    }
    match args.get(1).map(String::as_str) {
        Some("run-all") => driver::run_all(&args[2..]),
        Some("hold-work") => driver::hold_work(&args[2..]),
        Some("hold-directory") => {
            ensure!(args.len() == 2, "hold-directory takes no arguments");
            println!(
                "{}",
                serde_json::to_string(&lifecycle::Event::ProcessReady {
                    pid: std::process::id()
                })?
            );
            std::io::stdout().flush()?;
            let mut release = String::new();
            ensure!(
                std::io::stdin().read_line(&mut release)? > 0 && release.trim() == "finish",
                "directory holder release missing"
            );
            Ok(())
        }
        Some("finalize-boundary") => driver::finalize_boundary(&args[2..]),
        _ => bail!(
            "usage: issue-2806-build-drive run-all --evidence <directory> --assets <checkout> --amplihack <absolute binary> --runner <absolute binary> --cargo <absolute binary> --rustc <absolute binary> --precommit <absolute binary> --host-sentinel <host-only path> --tested-sha <sha>"
        ),
    }
}
fn fixture(name: &str, arguments: &[String]) -> Result<()> {
    let root = std::path::PathBuf::from(std::env::var("AMPLIHACK_2806_ROOT")?);
    let case = IsolatedCase::open(&root)?;
    if name == "model-launcher" {
        return launch_model(&case, arguments);
    }
    if name == "git-remote-issue2806" {
        return hosting::local_transport(&case, arguments);
    }
    let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
    if args == ["--version"] {
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(case.evidence.join("version-probes.jsonl"))?;
        writeln!(
            log,
            "{}",
            serde_json::json!({"client":name,"args":arguments,"pid":std::process::id()})
        )?;
        match name {
            "codex" => println!("codex-cli 0.98.0"),
            "claude" => println!("2.1.319 (Claude Code)"),
            _ => println!("gh version 2.70.0 (fixture)"),
        };
        return Ok(());
    }
    if name == "codex" && args.first() == Some(&"plugin") {
        println!("{{\"installed\":[]}}");
        return Ok(());
    }
    if let Some(directory) = hosting::option(&args, "--cd").or_else(|| hosting::option(&args, "-C"))
    {
        let directory = std::path::Path::new(directory);
        ensure!(
            directory.canonicalize()?.starts_with(&case.root),
            "agent cwd escapes case"
        );
        std::env::set_current_dir(directory)?;
    }
    if name == "gh" {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(case.evidence.join("hosting.lock"))?;
        fs4::fs_std::FileExt::lock_exclusive(&lock)?;
        let mut store = hosting::HostingStore::open(&case.evidence.join("hosting.json"))?;
        let mut owned = arguments.to_vec();
        if let Some(index) = owned.iter().position(|arg| arg == "--body-file")
            && owned.get(index + 1).is_some_and(|arg| arg == "-")
        {
            let mut body = String::new();
            std::io::stdin().read_to_string(&mut body)?;
            owned[index] = "--body".into();
            owned[index + 1] = body;
        }
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        let response = store.invoke(&std::env::current_dir()?, &borrowed)?;
        std::io::stdout().write_all(&response.stdout)?;
        std::io::stderr().write_all(&response.stderr)?;
        if response.exit_code != 0 {
            std::process::exit(response.exit_code);
        }
        return Ok(());
    }
    let mut invocations = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(case.evidence.join("model-transport.jsonl"))?;
    writeln!(
        invocations,
        "{}",
        serde_json::json!({"client":name,"args":arguments,"cwd":std::env::current_dir()?,"home":std::env::var("HOME")?,"pid":std::process::id()})
    )?;
    if name == "codex" {
        ensure!(
            args.contains(&"exec") && args.contains(&"-"),
            "unsupported Codex transport"
        );
    }
    let task_on_stdin =
        name == "codex" || (args.contains(&"--system-prompt-file") && args.contains(&"-p"));
    let task = if task_on_stdin {
        let mut task = String::new();
        std::io::stdin().read_to_string(&mut task)?;
        task
    } else if let Some(file) = hosting::option(&args, "--append-system-prompt-file") {
        let path = std::path::Path::new(file);
        let relative = path.strip_prefix(&case.root)?;
        std::fs::read_to_string(environment::confined_path(&case.root, relative)?)?
    } else {
        hosting::option(&args, "-p")
            .ok_or_else(|| anyhow::anyhow!("Claude task transport absent"))?
            .into()
    };
    let task = task
        .split_once("# Task instructions\n")
        .map_or(task.as_str(), |(_, task)| task)
        .trim_start();
    let response = responses::respond(&case, task)?;
    if name == "codex" {
        let path = hosting::option(&args, "--output-last-message")
            .ok_or_else(|| anyhow::anyhow!("Codex final-message path absent"))?;
        agent::write_codex_final(&case, std::path::Path::new(path), &response)?;
    } else {
        println!("{response}");
    }
    Ok(())
}

/// Transparent model-launch transport: execute the real CLI and retain its
/// stderr before runtime cleanup. Model task stdin and stdout pass unchanged.
fn launch_model(case: &IsolatedCase, arguments: &[String]) -> Result<()> {
    use std::process::{Command, Stdio};
    ensure!(
        matches!(
            arguments.first().map(String::as_str),
            Some("codex" | "claude")
        ),
        "unsupported model launch command"
    );
    let id = uuid::Uuid::new_v4();
    let mut log = std::fs::File::create(case.evidence.join(format!("launcher-{id}.stderr")))?;
    let mut child = Command::new(case.root.join("bin/amplihack"))
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()?;
    std::fs::write(
        case.evidence.join(format!("launcher-{id}.pid")),
        format!("{}\n", child.id()),
    )?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("launcher stderr missing"))?;
    let mut bytes = [0u8; 8192];
    loop {
        let size = stderr.read(&mut bytes)?;
        if size == 0 {
            break;
        }
        log.write_all(&bytes[..size])?;
        std::io::stderr().write_all(&bytes[..size])?;
    }
    let status = child.wait()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
