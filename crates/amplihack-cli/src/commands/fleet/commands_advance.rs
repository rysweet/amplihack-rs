use super::*;

pub(super) fn run_advance(
    vm: Option<&str>,
    session_target: Option<&str>,
    force: bool,
    save_path: Option<&Path>,
) -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    println!("Phase 1: Discovering fleet sessions...");
    let Some(discovery) = discover_scout_sessions(&azlin, vm, session_target, true)? else {
        return Ok(());
    };

    println!("\nPhase 2: Reasoning and executing actions...");
    let backend = NativeReasonerBackend::detect("auto")?;
    let mut reasoner = FleetSessionReasoner::new(azlin, backend);
    let mut decisions = Vec::<SessionDecisionRecord>::new();
    let mut executed = Vec::<SessionExecutionRecord>::new();

    for session in &discovery.sessions {
        println!(
            "\n  [{}/{}] reasoning...",
            session.vm_name, session.session_name
        );
        match reasoner.reason_about_session(
            &session.vm_name,
            &session.session_name,
            "",
            "",
            Some(&session.cached_tmux_capture),
        ) {
            Ok(analysis) => {
                let record = SessionDecisionRecord::from_analysis(&analysis);
                let decision = analysis.decision.clone();
                decisions.push(record.clone());
                match decision.action {
                    SessionAction::Wait | SessionAction::Escalate | SessionAction::MarkComplete => {
                        println!(
                            "    -> {} (no-op, conf={:.0}%)",
                            decision.action.as_str(),
                            decision.confidence * 100.0
                        );
                        executed.push(SessionExecutionRecord::skipped(&record, None));
                    }
                    SessionAction::SendInput => {
                        let preview = truncate_chars(&decision.input_text.replace('\n', " "), 60);
                        if !force
                            && !confirm_action(
                                &format!(
                                    "    -> send_input: \"{preview}\" (conf={:.0}%) Execute?",
                                    decision.confidence * 100.0
                                ),
                                true,
                            )?
                        {
                            println!("    Skipped.");
                            executed.push(SessionExecutionRecord::skipped(&record, None));
                            continue;
                        }
                        match reasoner.execute_decision(&decision) {
                            Ok(()) => {
                                println!(
                                    "    -> SENT: \"{preview}\" (conf={:.0}%)",
                                    decision.confidence * 100.0
                                );
                                executed.push(SessionExecutionRecord::executed(&record));
                            }
                            Err(error) => {
                                println!("    -> ERROR: {error}");
                                executed.push(SessionExecutionRecord::skipped(
                                    &record,
                                    Some(error.to_string()),
                                ));
                            }
                        }
                    }
                    SessionAction::Restart => {
                        if !force
                            && !confirm_action(
                                &format!(
                                    "    -> restart session (conf={:.0}%) Execute?",
                                    decision.confidence * 100.0
                                ),
                                false,
                            )?
                        {
                            println!("    Skipped.");
                            executed.push(SessionExecutionRecord::skipped(&record, None));
                            continue;
                        }
                        match reasoner.execute_decision(&decision) {
                            Ok(()) => {
                                println!(
                                    "    -> RESTARTED (conf={:.0}%)",
                                    decision.confidence * 100.0
                                );
                                executed.push(SessionExecutionRecord::executed(&record));
                            }
                            Err(error) => {
                                println!("    -> ERROR: {error}");
                                executed.push(SessionExecutionRecord::skipped(
                                    &record,
                                    Some(error.to_string()),
                                ));
                            }
                        }
                    }
                }
            }
            Err(error) => {
                println!("    -> ERROR: {error}");
                let record = SessionDecisionRecord {
                    vm: session.vm_name.clone(),
                    session: session.session_name.clone(),
                    status: session.status.as_str().to_string(),
                    branch: String::new(),
                    pr: String::new(),
                    action: "error".to_string(),
                    confidence: 0.0,
                    reasoning: String::new(),
                    input_text: String::new(),
                    error: Some(error.to_string()),
                    project: String::new(),
                    objectives: Vec::new(),
                };
                decisions.push(record.clone());
                executed.push(SessionExecutionRecord::skipped(
                    &record,
                    record.error.clone(),
                ));
            }
        }
    }

    println!("{}", render_advance_report(&decisions, &executed));
    if let Some(path) = save_path {
        let payload = serde_json::json!({
            "timestamp": now_isoformat(),
            "total_sessions": discovery.sessions.len(),
            "decisions": decisions,
            "executed": executed,
        });
        write_json_file(path, &payload)?;
        println!("\nJSON report saved to: {}", path.display());
    }
    Ok(())
}

pub(super) fn confirm_action(prompt: &str, default: bool) -> Result<bool> {
    use std::io::{self, Write};

    print!("{prompt} [{}] ", if default { "Y/n" } else { "y/N" });
    io::stdout()
        .flush()
        .context("failed to flush confirmation prompt")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read confirmation input")?;
    let trimmed = input.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Ok(default);
    }
    Ok(matches!(trimmed.as_str(), "y" | "yes"))
}

pub(super) fn default_last_scout_path() -> PathBuf {
    fleet_home_dir().join("last_scout.json")
}

pub(super) fn run_start(
    max_cycles: u32,
    interval: u64,
    adopt: bool,
    stuck_threshold: f64,
    max_agents_per_vm: usize,
    capture_lines: usize,
) -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let (mut admiral, existing_vms) = open_default_fleet_admiral(azlin)?;
    admiral.poll_interval_seconds = interval;
    admiral.max_agents_per_vm = max_agents_per_vm;
    admiral.observer.stuck_threshold_seconds = stuck_threshold;
    admiral.observer.capture_lines = capture_lines;

    if adopt {
        let adopted = admiral.adopt_all_sessions()?;
        if adopted > 0 {
            println!("Adopted {adopted} existing sessions");
        }
    }

    println!("Starting Fleet Admiral (Ctrl+C to stop)...");
    println!(
        "Poll interval: {}s, Max cycles: {}",
        interval,
        if max_cycles == 0 {
            "unlimited".to_string()
        } else {
            max_cycles.to_string()
        }
    );
    println!("Excluded VMs: {}", existing_vms.join(", "));
    println!();

    admiral.run_loop(max_cycles)?;
    Ok(())
}

pub(super) fn run_run_once() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let (mut admiral, _) = open_default_fleet_admiral(azlin)?;

    let actions = admiral.run_once()?;
    println!("Cycle completed: {} actions taken", actions.len());
    for action in actions {
        println!("  {}: {}", action.action_type.as_str(), action.reason);
    }
    Ok(())
}

pub(super) fn open_default_fleet_admiral(azlin: PathBuf) -> Result<(FleetAdmiral, Vec<String>)> {
    let queue = TaskQueue::load_default()?;
    let mut admiral = FleetAdmiral::new(azlin, queue, Some(default_log_dir()))?;
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    admiral.exclude_vms(&existing_refs);
    Ok((admiral, existing_vms))
}

pub(super) fn run_auth(vm_name: &str, services: &[String]) -> Result<()> {
    validate_vm_name(vm_name)?;
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let auth = AuthPropagator::new(azlin);
    let results = auth.propagate_all(vm_name, services);
    for result in &results {
        let status = if result.success { "OK" } else { "FAIL" };
        let files = if result.files_copied.is_empty() {
            "none".to_string()
        } else {
            result.files_copied.join(", ")
        };
        println!(
            "  [{status}] {}: {} ({:.1}s)",
            result.service, files, result.duration_seconds
        );
        if let Some(error) = &result.error {
            println!("         Error: {error}");
        }
    }

    println!("\nVerifying auth...");
    for (service, works) in auth.verify_auth(vm_name) {
        let icon = if works { '+' } else { 'X' };
        println!("  [{icon}] {service}");
    }

    Ok(())
}
