use super::*;

pub(super) fn run_tui(interval: u64, capture_lines: usize) -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let interval = interval.max(1);
    let capture_lines = capture_lines.clamp(1, MAX_CAPTURE_LINES);
    let mut ui_state = FleetTuiUiState::default();

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        println!("{}", render_tui_once(&azlin, interval, capture_lines)?);
        return Ok(());
    }

    // T4: Set up background channels and spawn fast/slow refresh threads.
    let (cmd_tx, cmd_rx) = mpsc::channel::<BackgroundCommand>();
    let (msg_tx, msg_rx) = mpsc::channel::<BackgroundMessage>();
    let shutdown_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Fast-status thread: wakes every TUI_FAST_REFRESH_INTERVAL_MS, processes
    // CreateSession commands, responds to ForceStatusRefresh requests.
    {
        let msg_tx = msg_tx.clone();
        let shutdown = Arc::clone(&shutdown_flag);
        let azlin_path = azlin.clone();
        thread::spawn(move || {
            let sleep_chunk = Duration::from_millis(50);
            let fast_interval = Duration::from_millis(TUI_FAST_REFRESH_INTERVAL_MS);
            let mut elapsed = Duration::ZERO;
            loop {
                if shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                // Drain commands.
                loop {
                    match cmd_rx.try_recv() {
                        Ok(BackgroundCommand::ForceStatusRefresh) => {
                            elapsed = fast_interval; // trigger immediate poll
                        }
                        Ok(BackgroundCommand::CreateSession {
                            azlin_path: ref ap,
                            ref vm_name,
                            ref agent,
                        }) => {
                            let result = background_create_session(ap, vm_name, agent);
                            let _ =
                                msg_tx.send(BackgroundMessage::SessionCreated { message: result });
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                thread::sleep(sleep_chunk);
                elapsed += sleep_chunk;
                if elapsed >= fast_interval {
                    elapsed = Duration::ZERO;
                    // Re-poll fleet state (best-effort; errors are non-fatal).
                    if let Ok(state) =
                        collect_observed_fleet_state(&azlin_path, DEFAULT_CAPTURE_LINES)
                    {
                        let _ = msg_tx.send(BackgroundMessage::FastStatusUpdate(state));
                    }
                }
            }
        });
    }

    // Slow-capture thread: wakes every TUI_SLOW_REFRESH_INTERVAL_MS and pushes
    // tmux captures for background sessions into the capture cache.
    {
        let msg_tx = msg_tx.clone();
        let shutdown = Arc::clone(&shutdown_flag);
        let azlin_path = azlin.clone();
        let capture_cache = Arc::clone(&ui_state.capture_cache);
        thread::spawn(move || {
            let sleep_chunk = Duration::from_millis(200);
            let slow_interval = Duration::from_millis(TUI_SLOW_REFRESH_INTERVAL_MS);
            let mut elapsed = slow_interval; // trigger first run immediately
            loop {
                if shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                thread::sleep(sleep_chunk);
                elapsed += sleep_chunk;
                if elapsed < slow_interval {
                    continue;
                }
                elapsed = Duration::ZERO;
                // Snapshot which sessions are currently cached so we can refresh
                // their captures in the background.
                let keys: Vec<(String, String)> = {
                    match capture_cache.lock() {
                        Ok(cache) => cache.iter().map(|(k, _)| k.clone()).collect(),
                        Err(e) => {
                            let _ = msg_tx.send(BackgroundMessage::Error(format!(
                                "capture cache lock poisoned: {e}"
                            )));
                            Vec::new()
                        }
                    }
                };
                for (vm_name, session_name) in keys {
                    if shutdown.load(std::sync::atomic::Ordering::Acquire) {
                        return;
                    }
                    if let Ok(output) = capture_tmux_output_with_timeout(
                        &azlin_path,
                        &vm_name,
                        &session_name,
                        DEFAULT_CAPTURE_LINES as u32,
                        CLI_WATCH_TIMEOUT,
                    ) {
                        let _ = msg_tx.send(BackgroundMessage::SlowCaptureUpdate {
                            vm_name,
                            session_name,
                            output,
                        });
                    }
                }
            }
        });
    }

    // Wire channels into ui_state.
    ui_state.bg_tx = Some(cmd_tx);
    ui_state.bg_rx = Some(Arc::new(Mutex::new(msg_rx)));

    let _terminal_guard = DashboardTerminalGuard::activate()?;
    let mut stdout = io::stdout();
    write!(stdout, "{HIDE_CURSOR}")?;
    stdout
        .flush()
        .context("failed to flush dashboard prelude")?;

    let result = (|| -> Result<()> {
        'dashboard: loop {
            // Drain any messages from background threads each frame.
            ui_state.drain_bg_messages();

            let state = collect_observed_fleet_state_with_progress(
                &azlin,
                capture_lines,
                |state, progress| {
                    ui_state.refresh_progress = Some(progress);
                    let frame = render_tui_frame(state, interval, &ui_state)?;
                    write!(stdout, "{CLEAR_SCREEN}{frame}")?;
                    stdout.flush().context("failed to flush dashboard frame")?;
                    Ok(())
                },
            )?;
            ui_state.refresh_progress = None;
            ui_state.sync_to_state(&state);
            let frame = render_tui_frame(&state, interval, &ui_state)?;
            write!(stdout, "{CLEAR_SCREEN}{frame}")?;
            stdout.flush().context("failed to flush dashboard frame")?;

            let mut refresh_detail_capture = false;
            let mut pending_key = read_dashboard_key(Duration::from_secs(interval));
            while let Some(key) = pending_key {
                match Some(key) {
                    Some(key) if ui_state.inline_input.is_some() => {
                        handle_tui_inline_input_key(&mut ui_state, key)?;
                        while ui_state.inline_input.is_some() {
                            let Some(extra_key) = read_dashboard_key(Duration::from_millis(0))
                            else {
                                break;
                            };
                            handle_tui_inline_input_key(&mut ui_state, extra_key)?;
                        }
                    }
                    Some(key) if ui_state.tab == FleetTuiTab::Editor && ui_state.editor_active => {
                        handle_tui_editor_active_key(&azlin, &mut ui_state, key)?;
                    }
                    // Quit
                    Some(DashboardKey::Char('q')) | Some(DashboardKey::Char('Q')) => {
                        break 'dashboard;
                    }
                    // Toggle help overlay
                    Some(DashboardKey::Char('?')) => ui_state.show_help = !ui_state.show_help,
                    Some(DashboardKey::Char('l')) | Some(DashboardKey::Char('L')) => {
                        ui_state.show_logo = !ui_state.show_logo;
                    }
                    Some(DashboardKey::Char('\u{1b}'))
                        if ui_state.tab == FleetTuiTab::Fleet
                            && ui_state.session_search.is_some() =>
                    {
                        ui_state.clear_session_search();
                    }
                    Some(DashboardKey::Char('\u{1b}'))
                    | Some(DashboardKey::Char('b'))
                    | Some(DashboardKey::Char('B'))
                        if !ui_state.show_help =>
                    {
                        ui_state.navigate_back();
                    }
                    // Force refresh — next loop re-collects state; detail also refreshes full capture.
                    Some(DashboardKey::Char('r')) | Some(DashboardKey::Char('R')) => {
                        refresh_detail_capture = ui_state.tab == FleetTuiTab::Detail;
                    }
                    // Navigation
                    Some(DashboardKey::Char('j'))
                    | Some(DashboardKey::Char('J'))
                    | Some(DashboardKey::Down) => {
                        ui_state.move_selection(&state, 1);
                        refresh_detail_capture = ui_state.tab == FleetTuiTab::Detail;
                    }
                    Some(DashboardKey::Char('k'))
                    | Some(DashboardKey::Char('K'))
                    | Some(DashboardKey::Up) => {
                        ui_state.move_selection(&state, -1);
                        refresh_detail_capture = ui_state.tab == FleetTuiTab::Detail;
                    }
                    // Tab cycling: Tab key = '\t' (forward), '[' = backward substitute.
                    Some(DashboardKey::Char('\t')) | Some(DashboardKey::Right) => {
                        ui_state.cycle_tab_forward();
                        refresh_detail_capture = ui_state.tab == FleetTuiTab::Detail;
                    }
                    Some(DashboardKey::Char('[')) | Some(DashboardKey::Left) => {
                        ui_state.cycle_tab_backward();
                        refresh_detail_capture = ui_state.tab == FleetTuiTab::Detail;
                    }
                    // Direct tab jumps
                    Some(DashboardKey::Char('1'))
                    | Some(DashboardKey::Char('f'))
                    | Some(DashboardKey::Char('F')) => ui_state.tab = FleetTuiTab::Fleet,
                    Some(DashboardKey::Char('2'))
                    | Some(DashboardKey::Char('s'))
                    | Some(DashboardKey::Char('S')) => {
                        ui_state.tab = FleetTuiTab::Detail;
                        refresh_detail_capture = true;
                    }
                    Some(DashboardKey::Char('\n')) | Some(DashboardKey::Char('\r'))
                        if ui_state.tab == FleetTuiTab::NewSession =>
                    {
                        run_tui_create_session(&azlin, &mut ui_state)?;
                    }
                    Some(DashboardKey::Char('\n')) | Some(DashboardKey::Char('\r')) => {
                        ui_state.tab = FleetTuiTab::Detail;
                        refresh_detail_capture = true;
                    }
                    Some(DashboardKey::Char('3')) => ui_state.tab = FleetTuiTab::Editor,
                    Some(DashboardKey::Char('4'))
                    | Some(DashboardKey::Char('p'))
                    | Some(DashboardKey::Char('P')) => ui_state.tab = FleetTuiTab::Projects,
                    // T6: 'n' cancels remove sub-mode. Must come before the 'n'/'N' tab-5 binding.
                    Some(DashboardKey::Char('n')) | Some(DashboardKey::Char('N'))
                        if ui_state.tab == FleetTuiTab::Projects
                            && ui_state.project_mode == ProjectManagementMode::Remove =>
                    {
                        ui_state.cancel_project_mode();
                    }
                    Some(DashboardKey::Char('5'))
                    | Some(DashboardKey::Char('n'))
                    | Some(DashboardKey::Char('N')) => ui_state.tab = FleetTuiTab::NewSession,
                    Some(DashboardKey::Char('/')) if ui_state.tab == FleetTuiTab::Fleet => {
                        ui_state.start_inline_session_search();
                        while ui_state.inline_input.is_some() {
                            let Some(extra_key) = read_dashboard_key(Duration::from_millis(0))
                            else {
                                break;
                            };
                            handle_tui_inline_input_key(&mut ui_state, extra_key)?;
                        }
                    }
                    Some(DashboardKey::Char('e')) => run_tui_edit(&mut ui_state),
                    Some(DashboardKey::Char('i')) | Some(DashboardKey::Char('I'))
                        if ui_state.tab == FleetTuiTab::Projects =>
                    {
                        run_tui_add_project(&mut ui_state)?;
                        while ui_state.inline_input.is_some() {
                            let Some(extra_key) = read_dashboard_key(Duration::from_millis(0))
                            else {
                                break;
                            };
                            handle_tui_inline_input_key(&mut ui_state, extra_key)?;
                        }
                    }
                    Some(DashboardKey::Char('i')) | Some(DashboardKey::Char('I')) => {
                        run_tui_edit_input(&mut ui_state)?;
                        while ui_state.inline_input.is_some() {
                            let Some(extra_key) = read_dashboard_key(Duration::from_millis(0))
                            else {
                                break;
                            };
                            handle_tui_inline_input_key(&mut ui_state, extra_key)?;
                        }
                    }
                    Some(DashboardKey::Char('t')) | Some(DashboardKey::Char('T'))
                        if ui_state.tab == FleetTuiTab::NewSession =>
                    {
                        ui_state.cycle_new_session_agent()
                    }
                    Some(DashboardKey::Char('t')) | Some(DashboardKey::Char('T'))
                        if ui_state.tab == FleetTuiTab::Fleet =>
                    {
                        ui_state.cycle_fleet_subview(&state)
                    }
                    Some(DashboardKey::Char('t')) | Some(DashboardKey::Char('T')) => {
                        ui_state.cycle_editor_action()
                    }
                    // T6: Projects tab — 'd' enters remove sub-mode, 'y' confirms,
                    // 'n' cancels. These guards must come before the catch-all 'd' arm.
                    Some(DashboardKey::Char('d')) | Some(DashboardKey::Char('D'))
                        if ui_state.tab == FleetTuiTab::Projects =>
                    {
                        if ui_state.project_mode == ProjectManagementMode::Remove {
                            run_tui_remove_project(&mut ui_state)?;
                            ui_state.confirm_project_remove();
                        } else {
                            ui_state.enter_project_remove_mode();
                        }
                    }
                    Some(DashboardKey::Char('y')) | Some(DashboardKey::Char('Y'))
                        if ui_state.tab == FleetTuiTab::Projects
                            && ui_state.project_mode == ProjectManagementMode::Remove =>
                    {
                        run_tui_remove_project(&mut ui_state)?;
                        ui_state.confirm_project_remove();
                    }
                    // Actions
                    Some(DashboardKey::Char('d')) | Some(DashboardKey::Char('D')) => {
                        run_tui_dry_run(&azlin, &state, &mut ui_state)?;
                        refresh_detail_capture = ui_state.tab == FleetTuiTab::Detail;
                    }
                    Some(DashboardKey::Char('a')) => {
                        run_tui_apply(&azlin, &mut ui_state)?;
                    }
                    Some(DashboardKey::Char('A')) if ui_state.tab == FleetTuiTab::Editor => {
                        run_tui_apply_edited(&azlin, &mut ui_state)?;
                    }
                    Some(DashboardKey::Char('A')) => {
                        run_tui_adopt_selected_session(&azlin, &state, &mut ui_state)?;
                    }
                    Some(DashboardKey::Char('x')) | Some(DashboardKey::Char('X'))
                        if ui_state.tab == FleetTuiTab::Projects =>
                    {
                        run_tui_remove_project(&mut ui_state)?;
                    }
                    Some(DashboardKey::Char('x')) | Some(DashboardKey::Char('X')) => {
                        ui_state.skip_selected_proposal();
                    }
                    // Status filters (toggle — press same key again to clear)
                    Some(DashboardKey::Char('E')) => ui_state.toggle_filter(StatusFilter::Errors),
                    Some(DashboardKey::Char('w')) | Some(DashboardKey::Char('W')) => {
                        ui_state.toggle_filter(StatusFilter::Waiting)
                    }
                    Some(DashboardKey::Char('c')) | Some(DashboardKey::Char('C')) => {
                        ui_state.toggle_filter(StatusFilter::Active)
                    }
                    // Clear filter
                    Some(DashboardKey::Char('*')) | Some(DashboardKey::Char('0')) => {
                        ui_state.status_filter = None
                    }
                    _ => {}
                }
                let frame = render_tui_frame(&state, interval, &ui_state)?;
                write!(stdout, "{CLEAR_SCREEN}{frame}")?;
                stdout.flush().context("failed to flush dashboard frame")?;
                pending_key = read_dashboard_key(Duration::from_millis(0));
            }

            if refresh_detail_capture {
                run_tui_refresh_detail_capture(
                    &azlin,
                    &state,
                    &mut ui_state,
                    capture_lines as u32,
                )?;
            }
        }
        Ok(())
    })();

    // T4: Signal background threads to stop.
    shutdown_flag.store(true, std::sync::atomic::Ordering::Release);
    // Drop the sender so the fast thread's cmd_rx sees Disconnected.
    drop(ui_state.bg_tx.take());

    writeln!(stdout, "{SHOW_CURSOR}")?;
    stdout
        .flush()
        .context("failed to flush dashboard teardown")?;
    result
}

