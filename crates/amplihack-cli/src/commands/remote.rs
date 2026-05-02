use std::str::FromStr;

use amplihack_remote::{
    CommandMode, ExecOptions, KillOptions, ListOptions, OutputOptions, RemoteError, SessionStatus,
    StartOptions, StatusOptions, VMOptions, VMSize, capture_output, exec, kill_session,
    list_sessions, start_sessions, status,
};
use anyhow::{Context, Result};

use crate::RemoteCommands;

pub fn run(command: RemoteCommands) -> Result<()> {
    let runtime = tokio_runtime()?;
    match command {
        RemoteCommands::Exec {
            command,
            prompt,
            max_turns,
            vm_size,
            vm_name,
            keep_vm,
            no_reuse,
            timeout,
            region,
            port,
            azlin_args,
        } => {
            let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
            let result = runtime.block_on(exec(ExecOptions {
                repo_path: std::env::current_dir()?,
                command: CommandMode::from_str(&command).map_err(anyhow::Error::msg)?,
                prompt,
                max_turns,
                vm_options: VMOptions {
                    size: vm_size,
                    region,
                    vm_name,
                    no_reuse,
                    keep_vm,
                    azlin_extra_args: (!azlin_args.is_empty()).then_some(azlin_args),
                    tunnel_port: port,
                },
                timeout_minutes: timeout,
                skip_secret_scan: false,
                api_key,
            }));
            let result = handle_remote_result(result)?;
            if let Some(vm) = result.vm_name {
                println!("VM: {vm}");
            }
            if let Some(summary) = result.summary {
                println!("Branches: {}", summary.branches.len());
                println!("Commits: {}", summary.commits_count);
                println!("Files changed: {}", summary.files_changed);
            }
            if result.exit_code != 0 {
                std::process::exit(result.exit_code);
            }
            Ok(())
        }
        RemoteCommands::List { status, json } => {
            let status = status
                .as_deref()
                .map(SessionStatus::from_str)
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let sessions = handle_remote_result(list_sessions(ListOptions {
                status,
                state_file: None,
            }))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
                return Ok(());
            }
            if sessions.is_empty() {
                println!("No remote sessions found.");
                return Ok(());
            }
            println!(
                "{:<30} {:<32} {:<10} {:<8} PROMPT",
                "SESSION", "VM", "STATUS", "AGE"
            );
            println!("{}", "-".repeat(120));
            let now = chrono::Utc::now();
            for session in &sessions {
                let age_minutes = (now - session.created_at).num_minutes();
                let age = if age_minutes < 60 {
                    format!("{}m", age_minutes.max(0))
                } else {
                    format!("{}h", age_minutes / 60)
                };
                let prompt = truncate(&session.prompt, 50);
                println!(
                    "{:<30} {:<32} {:<10} {:<8} {}",
                    session.session_id, session.vm_name, session.status, age, prompt
                );
            }
            println!("\nTotal: {} session(s)", sessions.len());
            Ok(())
        }
        RemoteCommands::Start {
            prompts,
            command,
            max_turns,
            size,
            region,
            port,
        } => {
            let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
            let summary = runtime.block_on(start_sessions(StartOptions {
                repo_path: std::env::current_dir()?,
                prompts,
                command: CommandMode::from_str(&command).map_err(anyhow::Error::msg)?,
                max_turns,
                size: VMSize::from_str(&size).map_err(anyhow::Error::msg)?,
                region: Some(region),
                tunnel_port: port,
                api_key,
                state_file: None,
            }));
            let summary = handle_remote_result(summary)?;
            println!(
                "Successfully started {} session(s):",
                summary.session_ids.len()
            );
            for session_id in summary.session_ids {
                println!("  - {session_id}");
            }
            println!("Use 'amplihack remote output <session-id>' to view progress");
            Ok(())
        }
        RemoteCommands::Output {
            session_id,
            lines,
            follow,
        } => loop {
            let output = runtime.block_on(capture_output(OutputOptions {
                session_id: session_id.clone(),
                lines,
                state_file: None,
            }));
            let output = handle_remote_result(output)?;
            if follow {
                print!("\x1B[2J\x1B[1;1H");
            }
            println!("=== Session: {} ===", output.session.session_id);
            println!("Status: {}", output.session.status);
            println!("VM: {}", output.session.vm_name);
            println!("Prompt: {}", output.session.prompt);
            println!("{}", "=".repeat(80));
            println!("{}", output.output);
            if !follow {
                break Ok(());
            }
            println!("\n[Following output... Press Ctrl+C to stop]");
            std::thread::sleep(std::time::Duration::from_secs(5));
        },
        RemoteCommands::Kill { session_id, force } => {
            handle_remote_result(runtime.block_on(kill_session(KillOptions {
                session_id: session_id.clone(),
                force,
                state_file: None,
            })))?;
            println!("Session '{session_id}' has been terminated.");
            Ok(())
        }
        RemoteCommands::Status { json } => {
            let remote_status = handle_remote_result(status(StatusOptions { state_file: None }))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&remote_status)?);
                return Ok(());
            }
            println!("\n=== Remote Session Pool Status ===\n");
            println!("VMs: {} total", remote_status.pool.total_vms);
            if remote_status.vms.is_empty() {
                println!("  (No VMs in pool)");
            } else {
                for entry in remote_status.vms {
                    let capacity_pct = (entry.active_sessions.len() * 100)
                        .checked_div(entry.capacity)
                        .unwrap_or(0);
                    println!("  {} ({}, {})", entry.vm.name, entry.vm.size, entry.region);
                    println!(
                        "    Sessions: {}/{} ({}% capacity)",
                        entry.active_sessions.len(),
                        entry.capacity,
                        capacity_pct
                    );
                }
            }
            println!("\nSessions: {} total", remote_status.total_sessions);
            println!("  Running: {}", remote_status.sessions.running);
            println!("  Completed: {}", remote_status.sessions.completed);
            println!("  Failed: {}", remote_status.sessions.failed);
            println!("  Killed: {}", remote_status.sessions.killed);
            println!("  Pending: {}", remote_status.sessions.pending);
            Ok(())
        }
    }
}

fn handle_remote_result<T>(result: std::result::Result<T, RemoteError>) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(RemoteError::SessionNotFound { session_id }) => {
            eprintln!("Error: Session '{session_id}' not found.");
            eprintln!("Use 'amplihack remote list' to see available sessions.");
            std::process::exit(3);
        }
        Err(err) => Err(anyhow::Error::new(err)),
    }
}

fn tokio_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create Tokio runtime")
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let mut result: String = value.chars().take(max).collect();
        result.push_str("...");
        result
    }
}
