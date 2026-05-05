use super::*;

pub(super) fn run_scout(
    vm: Option<&str>,
    session_target: Option<&str>,
    skip_adopt: bool,
    incremental: bool,
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
    let Some(discovery) = discover_scout_sessions(&azlin, vm, session_target, false)? else {
        return Ok(());
    };

    let mut adopted_count = 0usize;
    if !skip_adopt {
        println!("\nPhase 2: Adopting sessions...");
        let mut queue = TaskQueue::load_default()?;
        let adopter = SessionAdopter::new(azlin.clone());
        let mut vm_sessions = BTreeMap::<String, Vec<String>>::new();
        for session in &discovery.sessions {
            vm_sessions
                .entry(session.vm_name.clone())
                .or_default()
                .push(session.session_name.clone());
        }

        for (vm_name, session_names) in vm_sessions {
            match adopter.adopt_sessions(&vm_name, &mut queue, Some(&session_names)) {
                Ok(adopted) => {
                    adopted_count += adopted.len();
                    if !adopted.is_empty() {
                        println!("  {vm_name}: adopted {} sessions", adopted.len());
                    }
                }
                Err(error) => println!("  {vm_name}: adoption error -- {error}"),
            }
        }
        println!("Total adopted: {adopted_count}");
    } else {
        println!("\nPhase 2: Skipped (--skip-adopt)");
    }

    let mut previous_statuses = BTreeMap::<String, String>::new();
    let mut previous_decisions = Vec::<SessionDecisionRecord>::new();
    if incremental {
        let last_scout_path = default_last_scout_path();
        if last_scout_path.exists() {
            match load_default_previous_scout() {
                Ok((statuses, decisions)) => {
                    previous_statuses = statuses;
                    previous_decisions = decisions;
                    println!(
                        "\nIncremental mode: loaded {} previous statuses",
                        previous_statuses.len()
                    );
                }
                Err(_) => {
                    println!("\nIncremental mode: could not load previous scout, running full");
                }
            }
        }
    }

    println!("\nPhase 3: Reasoning about sessions...");
    let backend = NativeReasonerBackend::detect("auto")?;
    let mut reasoner = FleetSessionReasoner::new(azlin, backend);
    let mut decisions = Vec::<SessionDecisionRecord>::new();

    for session in &discovery.sessions {
        let session_key = format!("{}/{}", session.vm_name, session.session_name);
        let session_status = session.status.as_str().to_string();
        if incremental && previous_statuses.get(&session_key) == Some(&session_status) {
            println!("  Skipping (unchanged): {session_key} [{session_status}]");
            if let Some(previous) = previous_decisions.iter().find(|decision| {
                decision.vm == session.vm_name && decision.session == session.session_name
            }) {
                decisions.push(previous.clone());
            } else {
                decisions.push(SessionDecisionRecord {
                    vm: session.vm_name.clone(),
                    session: session.session_name.clone(),
                    status: session_status,
                    branch: String::new(),
                    pr: String::new(),
                    action: SessionAction::Wait.as_str().to_string(),
                    confidence: 0.5,
                    reasoning: "Unchanged since last scout".to_string(),
                    input_text: String::new(),
                    error: None,
                    project: String::new(),
                    objectives: Vec::new(),
                });
            }
            continue;
        }

        println!(
            "  Reasoning: {}/{}...",
            session.vm_name, session.session_name
        );
        match reasoner.reason_about_session(
            &session.vm_name,
            &session.session_name,
            "",
            "",
            Some(&session.cached_tmux_capture),
        ) {
            Ok(analysis) => decisions.push(SessionDecisionRecord::from_analysis(&analysis)),
            Err(error) => decisions.push(SessionDecisionRecord {
                vm: session.vm_name.clone(),
                session: session.session_name.clone(),
                status: session_status,
                branch: String::new(),
                pr: String::new(),
                action: "error".to_string(),
                confidence: 0.0,
                reasoning: String::new(),
                input_text: String::new(),
                error: Some(error.to_string()),
                project: String::new(),
                objectives: Vec::new(),
            }),
        }
    }

    println!(
        "{}",
        render_scout_report(
            &decisions,
            discovery.all_vm_count,
            discovery.running_vm_count,
            adopted_count,
            skip_adopt
        )
    );

    let snapshot = LastScoutSnapshot::new(
        discovery.running_vm_count,
        discovery.sessions.len(),
        adopted_count,
        skip_adopt,
        decisions.clone(),
        &discovery.sessions,
    );
    snapshot.save_default()?;
    if let Some(path) = save_path {
        snapshot.save(path)?;
        println!("\nJSON report saved to: {}", path.display());
    }
    Ok(())
}
