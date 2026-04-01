use super::*;

pub(super) fn terminal_cols() -> usize {
    #[cfg(unix)]
    {
        let cols = unsafe {
            let mut ws = libc::winsize {
                ws_row: 0,
                ws_col: 0,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                ws.ws_col as usize
            } else {
                80
            }
        };
        cols.clamp(40, 100)
    }
    #[cfg(not(unix))]
    {
        80
    }
}

/// Wrap `content` in a double-rule box-drawing vertical border.
///
/// `visible_len` is the printable (non-ANSI) character count of `content`.
/// `inner` is the usable width inside the box borders (excluding the 2-char
/// border+space on each side).
pub(super) fn cockpit_boxline(content: &str, visible_len: usize, inner: usize) -> String {
    let pad = inner.saturating_sub(visible_len);
    format!(
        "{ANSI_BOLD}{BOX_VL}{ANSI_RESET} {content}{}{ANSI_BOLD}{BOX_VL}{ANSI_RESET}",
        " ".repeat(pad),
    )
}

/// Return the ANSI color string and Unicode status icon for a session status.
pub(super) fn status_color_and_icon(status: AgentStatus) -> (&'static str, &'static str) {
    match status {
        AgentStatus::Running | AgentStatus::Thinking => (ANSI_GREEN, "\u{25c9}"), // ◉
        AgentStatus::WaitingInput => (ANSI_CYAN, "\u{25c9}"),
        AgentStatus::Idle => (ANSI_YELLOW, "\u{25cf}"), // ●
        AgentStatus::Shell => (ANSI_DIM, "\u{25cb}"),   // ○
        AgentStatus::Completed => (ANSI_BLUE, "\u{2713}"), // ✓
        AgentStatus::Error | AgentStatus::Stuck => (ANSI_RED, "\u{2717}"), // ✗
        _ => (ANSI_DIM, "\u{25cb}"),
    }
}

pub(super) fn render_tui_frame(
    state: &FleetState,
    interval: u64,
    ui_state: &FleetTuiUiState,
) -> Result<String> {
    let (total, active, waiting, errors, idle) =
        fleet_status_summary(state, ui_state.fleet_subview);

    // Terminal dimensions.
    let cols = terminal_cols();
    let width = cols.saturating_sub(2); // outer border consumes 1 col each side
    let inner = width.saturating_sub(4); // 2 for border+space on each side

    // Wall-clock timestamp (seconds precision, UTC-local).
    let secs_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let s = secs_since_epoch % 60;
    let m = (secs_since_epoch / 60) % 60;
    let h = (secs_since_epoch / 3600) % 24;
    let now = format!("{h:02}:{m:02}:{s:02}");

    // ---------- title row ---------------------------------------------------
    let title_text = "FLEET DASHBOARD";
    let total_vms = FleetTuiUiState::fleet_vms(state, ui_state.fleet_subview).len();
    let stats_text = format!("Updated: {now}    [{total_vms} VMs / {total} sessions]");
    let title_raw = 2 + title_text.len();
    let stats_raw = stats_text.len();
    let gap = inner.saturating_sub(title_raw + stats_raw);
    let title_line = format!(
        "  {ANSI_BOLD}{title_text}{ANSI_RESET}{}  {ANSI_DIM}{stats_text}{ANSI_RESET}",
        " ".repeat(gap)
    );

    // ---------- tab bar -------------------------------------------------------
    let tab_labels: Vec<String> = [
        FleetTuiTab::Fleet,
        FleetTuiTab::Detail,
        FleetTuiTab::Editor,
        FleetTuiTab::Projects,
        FleetTuiTab::NewSession,
    ]
    .iter()
    .map(|t| {
        let lbl = t.label();
        if *t == ui_state.tab {
            format!("{ANSI_BOLD}{ANSI_CYAN}[{lbl}]{ANSI_RESET}")
        } else {
            format!(" {lbl} ")
        }
    })
    .collect();
    let tab_bar_raw: usize = [
        FleetTuiTab::Fleet,
        FleetTuiTab::Detail,
        FleetTuiTab::Editor,
        FleetTuiTab::Projects,
        FleetTuiTab::NewSession,
    ]
    .iter()
    .map(|t| t.label().len() + 2)
    .sum::<usize>()
        + 4 * 3; // 4 " | " separators

    // ---------- status summary line ------------------------------------------
    let filter_hint = ui_state.fleet_filter_summary();
    let refresh_hint = ui_state
        .refresh_progress
        .as_ref()
        .map(|progress| format!("  [refresh: {}]", progress.label()))
        .unwrap_or_default();
    // Unicode status icons used inline (must be string literals in format! args).
    let icon_filled = "\u{25c9}"; // ◉
    let icon_circle = "\u{25cf}"; // ●
    let icon_cross = "\u{2717}"; // ✗
    let status_parts = format!(
        "{ANSI_GREEN}{icon_filled} active: {active}{ANSI_RESET}  \
         {ANSI_YELLOW}{icon_circle} idle: {idle}{ANSI_RESET}  \
         {ANSI_CYAN}{icon_filled} waiting: {waiting}{ANSI_RESET}  \
         {ANSI_RED}{icon_cross} error: {errors}{ANSI_RESET}{filter_hint}{refresh_hint}"
    );
    let status_raw =
        format!(
            "active: {active}  idle: {idle}  waiting: {waiting}  error: {errors}{filter_hint}{refresh_hint}"
        )
            .len()
            + 4 * 2; // icon + space per segment

    // ---------- controls line -------------------------------------------------
    let controls_text = format!(
        "  q quit  b back  r refresh  l logo  t view/action  d dry-run  a apply  A adopt/apply-edited  ? help  ({}s)",
        interval.max(1)
    );
    let controls_raw = controls_text.len();
    let controls_line = format!("{ANSI_DIM}{controls_text}{ANSI_RESET}");

    // ---------- borders -------------------------------------------------------
    let hl_str: String = std::iter::repeat_n(BOX_HL, width.saturating_sub(2)).collect();
    let top_border = format!("{ANSI_BOLD}{BOX_TL}{hl_str}{BOX_TR}{ANSI_RESET}");
    let sep = format!("{ANSI_BOLD}{BOX_ML}{hl_str}{BOX_MR}{ANSI_RESET}");
    let bot_border = format!("{ANSI_BOLD}{BOX_BL}{hl_str}{BOX_BR}{ANSI_RESET}");

    let mut lines: Vec<String> = vec![top_border.clone()];
    lines.push(cockpit_boxline(
        &title_line,
        title_raw + stats_raw + gap + 2,
        inner,
    ));
    if ui_state.show_logo {
        for logo_line in fleet_logo_lines() {
            lines.push(cockpit_boxline(logo_line, logo_line.chars().count(), inner));
        }
        lines.push(cockpit_boxline("", 0, inner));
    }
    lines.push(sep.clone());
    lines.push(cockpit_boxline(&tab_labels.join(" | "), tab_bar_raw, inner));
    lines.push(cockpit_boxline(&status_parts, status_raw, inner));
    lines.push(cockpit_boxline(&controls_line, controls_raw, inner));
    lines.push(cockpit_boxline("", 0, inner));

    // ---------- error banner --------------------------------------------------
    if errors > 0 {
        let banner_text = format!(
            "!! WARNING: {errors} session(s) in ERROR/STUCK state — press 'E' to filter !!"
        );
        let banner_raw = banner_text.len();
        let banner = format!("{ANSI_RED}{ANSI_BOLD}{banner_text}{ANSI_RESET}");
        lines.push(cockpit_boxline(&banner, banner_raw, inner));
        lines.push(cockpit_boxline("", 0, inner));
    }

    lines.push(sep.clone());

    // ---------- content area --------------------------------------------------
    if ui_state.show_help {
        cockpit_render_help_overlay(&mut lines, inner);
    } else {
        match ui_state.tab {
            FleetTuiTab::Fleet => cockpit_render_fleet_view(state, ui_state, &mut lines, inner),
            FleetTuiTab::Detail => cockpit_render_detail_view(state, ui_state, &mut lines, inner),
            FleetTuiTab::Projects => cockpit_render_projects_view(ui_state, &mut lines, inner)?,
            FleetTuiTab::Editor => cockpit_render_editor_view(ui_state, &mut lines, inner),
            FleetTuiTab::NewSession => {
                cockpit_render_new_session_view(state, ui_state, &mut lines, inner)
            }
        }
    }

    // ---------- status bar ----------------------------------------------------
    if let Some(message) = &ui_state.status_message {
        lines.push(sep.clone());
        lines.push(cockpit_boxline(message, message.len(), inner));
    }

    lines.push(bot_border);
    Ok(lines.join("\n"))
}

pub(super) fn fleet_logo_lines() -> &'static [&'static str] {
    &[
        "              _~",
        "             /~   \\",
        "            |  ☠  |",
        "             \\_~_/",
        "        |    |    |",
        "        )_)  )_)  )_)",
        "       )___))___))___)\\",
        "      )____)____)_____)\\\\",
        "    _____|____|____|____\\\\\\__",
        "---\\                   /------",
        "    \\_________________/",
        "~~~  A M P L I H A C K   F L E E T  ~~~",
    ]
}

// ── Cockpit boxed-content render helpers ─────────────────────────────────────

pub(super) fn cockpit_render_help_overlay(lines: &mut Vec<String>, inner: usize) {
    let mut push = |text: &str| {
        lines.push(cockpit_boxline(text, text.len(), inner));
    };
    push("KEYBINDING HELP");
    push("");
    push("Navigation");
    push("  q / Q          Quit the dashboard");
    push("  r / R          Force refresh now");
    push("  j / J / Down   Move selection down");
    push("  k / K / Up     Move selection up");
    push("  Tab / Right    Cycle tabs forward");
    push("  [ / Left       Cycle tabs backward");
    push("  1 / f / F      Jump to Fleet tab");
    push("  2 / s / S      Jump to Detail tab");
    push("  3              Jump to Editor tab");
    push("  4 / p / P      Jump to Projects tab");
    push("  5 / n / N      Jump to New Session tab");
    push("  Esc / b / B    Back: editor->detail, detail/projects->fleet");
    push("");
    push("Actions");
    push("  e              Load selected proposal into the editor");
    push("  i / I          Edit editor input or add a project repo (projects)");
    push("  t / T          Cycle fleet subview, editor action, or new-session agent type");
    push("  d / D          Dry-run reasoner on selected session");
    push("  a              Apply last prepared proposal to session");
    push("  A              Adopt selected session (fleet) or apply edited proposal (editor)");
    push("  x / X          Skip proposal or remove selected project (projects)");
    push("  Enter          Open detail tab or create new session");
    push("  l / L          Toggle fleet logo");
    push("  /              Search fleet sessions by VM or session name");
    push("  Esc            Clear fleet search (fleet) or go back");
    push("");
    push("Filters — fleet view (press same key again to clear)");
    push("  E              Show only Error/Stuck sessions");
    push("  w / W          Show only WaitingInput sessions");
    push("  c / C          Show only Active (Running/Thinking) sessions");
    push("  * / 0          Clear all filters");
    push("");
    push("  ?              Toggle this help overlay");
}

