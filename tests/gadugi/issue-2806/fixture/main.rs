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
    if matches!(name, "gh" | "claude" | "codex" | "git-remote-issue2806") {
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
            "usage: issue-2806-build-drive run-all --evidence <directory> --assets <checkout> --amplihack <absolute binary> --runner <absolute binary> --cargo <absolute binary> --rustc <absolute binary> --precommit <absolute binary> --tested-sha <sha>"
        ),
    }
}
fn fixture(name: &str, arguments: &[String]) -> Result<()> {
    let root = std::path::PathBuf::from(std::env::var("AMPLIHACK_2806_ROOT")?);
    let case = IsolatedCase::open(&root)?;
    if name == "git-remote-issue2806" {
        return hosting::local_transport(&case, arguments);
    }
    let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
    if args == ["--version"] {
        let version = if name == "claude" {
            "2.1.319"
        } else {
            "0.166.0"
        };
        println!("{name} {version} (issue-2806 fixture)");
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
    let task = if name == "codex" {
        ensure!(
            args.contains(&"exec") && args.contains(&"-"),
            "unsupported Codex transport"
        );
        let mut task = String::new();
        std::io::stdin().read_to_string(&mut task)?;
        task
    } else if args.contains(&"--system-prompt-file") && args.contains(&"-p") {
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
