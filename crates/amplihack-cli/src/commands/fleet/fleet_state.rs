use super::*;

#[derive(Debug, Clone)]
pub(super) struct FleetState {
    pub(super) vms: Vec<VmInfo>,
    pub(super) timestamp: Option<DateTime<Local>>,
    pub(super) azlin_path: PathBuf,
    pub(super) exclude_vms: Vec<String>,
}

impl FleetState {
    pub(super) fn new(azlin_path: PathBuf) -> Self {
        Self {
            vms: Vec::new(),
            timestamp: None,
            azlin_path,
            exclude_vms: Vec::new(),
        }
    }

    pub(super) fn exclude_vms(&mut self, vm_names: &[&str]) {
        self.exclude_vms
            .extend(vm_names.iter().map(|name| (*name).to_string()));
    }

    pub(super) fn refresh(&mut self) {
        self.refresh_inventory();
        let azlin_path = self.azlin_path.clone();
        let excluded = self.exclude_vms.clone();

        for vm in &mut self.vms {
            if vm.is_running() && !excluded.iter().any(|name| name == &vm.name) {
                vm.tmux_sessions = Self::poll_tmux_sessions_with_path(&azlin_path, &vm.name);
            }
        }
    }

    pub(super) fn refresh_inventory(&mut self) {
        self.vms = self.poll_vms();
        self.timestamp = Some(Local::now());
    }

    pub(super) fn summary(&self) -> String {
        let managed: Vec<&VmInfo> = self
            .vms
            .iter()
            .filter(|vm| !self.exclude_vms.iter().any(|name| name == &vm.name))
            .collect();
        let running = managed.iter().filter(|vm| vm.is_running()).count();
        let sessions = managed
            .iter()
            .map(|vm| vm.tmux_sessions.len())
            .sum::<usize>();
        let agents = managed.iter().map(|vm| vm.active_agents()).sum::<usize>();

        let mut lines = vec![match &self.timestamp {
            Some(timestamp) => format!("Fleet State ({})", timestamp.format("%Y-%m-%d %H:%M:%S")),
            None => "Fleet State".to_string(),
        }];
        lines.push(format!(
            "  Total VMs: {} ({} managed, {} excluded)",
            self.vms.len(),
            managed.len(),
            self.exclude_vms.len()
        ));
        lines.push(format!("  Running: {running}"));
        lines.push(format!("  Tmux sessions: {sessions}"));
        lines.push(format!("  Active agents: {agents}"));
        lines.push(String::new());

        for vm in managed {
            let status_icon = if vm.is_running() { '+' } else { '-' };
            lines.push(format!(
                "  [{status_icon}] {} ({}) - {}",
                vm.name, vm.region, vm.status
            ));
            for session in &vm.tmux_sessions {
                lines.push(format!(
                    "    [{}] {} ({})",
                    session.agent_status.summary_icon(),
                    session.session_name,
                    session.agent_status.as_str()
                ));
            }
        }

        lines.join("\n")
    }

    pub(super) fn managed_vms(&self) -> Vec<&VmInfo> {
        self.vms
            .iter()
            .filter(|vm| !self.exclude_vms.iter().any(|name| name == &vm.name))
            .collect()
    }

    pub(super) fn all_vms(&self) -> Vec<&VmInfo> {
        self.vms.iter().collect()
    }

    /// Returns `true` if `vm_name` is not in the exclude list.
    ///
    /// Used for managed/unmanaged labeling in the AllSessions subview.
    pub(super) fn is_managed_vm(&self, vm_name: &str) -> bool {
        !self.exclude_vms.iter().any(|name| name == vm_name)
    }

    pub(super) fn idle_vms(&self) -> Vec<&VmInfo> {
        self.managed_vms()
            .into_iter()
            .filter(|vm| vm.is_running() && vm.active_agents() == 0)
            .collect()
    }

    pub(super) fn get_vm(&self, vm_name: &str) -> Option<&VmInfo> {
        self.vms.iter().find(|vm| vm.name == vm_name)
    }

    pub(super) fn poll_vms(&self) -> Vec<VmInfo> {
        let azlin = self.azlin_path.display().to_string();

        // Preferred path: structured JSON inventory.
        let mut json_cmd = Command::new(&self.azlin_path);
        json_cmd.args(["list", "--json"]);
        let json_fallback_reason: Option<String> =
            match run_output_with_timeout(json_cmd, AZLIN_LIST_TIMEOUT) {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.trim().is_empty() {
                        Some("`azlin list --json` produced empty output".to_string())
                    } else {
                        let parsed = Self::parse_vm_json(&stdout);
                        if !parsed.is_empty() || stdout.trim() == "[]" {
                            // Success — including a genuinely empty `[]` fleet,
                            // which is a real answer and stays silent.
                            return parsed;
                        }
                        Some(
                            "`azlin list --json` output was not parseable as a VM list".to_string(),
                        )
                    }
                }
                Ok(output) => Some(format!(
                    "`azlin list --json` exited with status {}",
                    output.status
                )),
                Err(e) => Some(format!("`azlin list --json` failed to run: {e}")),
            };

        // We only reach here if JSON did not yield a usable inventory. Surface
        // the transition so a failed/degraded query is distinguishable from an
        // empty fleet before we retry with the text parser.
        if let Some(reason) = &json_fallback_reason {
            tracing::warn!(
                azlin = %azlin,
                reason = %reason,
                "azlin JSON inventory unavailable; falling back to text parser"
            );
        }

        let mut text_cmd = Command::new(&self.azlin_path);
        text_cmd.arg("list");
        match run_output_with_timeout(text_cmd, AZLIN_LIST_TIMEOUT) {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Self::parse_vm_text(&stdout)
            }
            Ok(output) => {
                tracing::error!(
                    azlin = %azlin,
                    status = %output.status,
                    "azlin list failed (non-zero exit); reporting an empty fleet here is unsafe — treat as a query failure, not 'no VMs' (risks double-provisioning / orphans)"
                );
                Vec::new()
            }
            Err(e) => {
                tracing::error!(
                    azlin = %azlin,
                    error = %e,
                    "azlin list failed to run (timeout / missing binary / spawn error); reporting an empty fleet here is unsafe — treat as a query failure, not 'no VMs' (risks double-provisioning / orphans)"
                );
                Vec::new()
            }
        }
    }

    pub(super) fn parse_vm_json(json_str: &str) -> Vec<VmInfo> {
        let value: Value = match serde_json::from_str(json_str) {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };

        let items = if let Some(list) = value.as_array() {
            list.to_vec()
        } else if let Some(list) = value.get("vms").and_then(Value::as_array) {
            list.to_vec()
        } else {
            Vec::new()
        };

        items
            .into_iter()
            .map(|item| {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let session_name = item
                    .get("session_name")
                    .and_then(Value::as_str)
                    .unwrap_or(&name)
                    .to_string();
                let region = item
                    .get("region")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("location").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();

                VmInfo {
                    name,
                    session_name,
                    os: item
                        .get("os")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    ip: item
                        .get("ip")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    region,
                    tmux_sessions: Vec::new(),
                }
            })
            .collect()
    }

    pub(super) fn parse_vm_text(text: &str) -> Vec<VmInfo> {
        let mut vms = Vec::new();
        let mut in_table = false;

        for line in text.lines() {
            if line.contains("Session") && line.contains("Tmux") {
                in_table = true;
                continue;
            }
            if line.starts_with('┣') || line.starts_with('┡') || line.starts_with('└') {
                continue;
            }
            if !in_table || !line.contains('│') {
                continue;
            }

            let parts = line
                .split('│')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            if parts.len() < 4 || parts[0].is_empty() {
                continue;
            }

            vms.push(VmInfo {
                name: parts[0].to_string(),
                session_name: parts[0].to_string(),
                os: parts.get(2).copied().unwrap_or("").to_string(),
                status: parts.get(3).copied().unwrap_or("").to_string(),
                ip: parts.get(4).copied().unwrap_or("").to_string(),
                region: parts.get(5).copied().unwrap_or("").to_string(),
                tmux_sessions: Vec::new(),
            });
        }

        vms
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn poll_tmux_sessions(&self, vm_name: &str) -> Vec<TmuxSessionInfo> {
        Self::poll_tmux_sessions_with_path(&self.azlin_path, vm_name)
    }

    pub(super) fn poll_tmux_sessions_with_path(
        azlin_path: &Path,
        vm_name: &str,
    ) -> Vec<TmuxSessionInfo> {
        let mut cmd = Command::new(azlin_path);
        cmd.args([
            "connect",
            vm_name,
            "--no-tmux",
            "--",
            "tmux list-sessions -F '#{session_name}|||#{session_windows}|||#{session_attached}' 2>/dev/null || echo 'no-tmux'",
        ]);

        match run_output_with_timeout(cmd, TMUX_LIST_TIMEOUT) {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !output.status.success() || stdout.contains("no-tmux") {
                    return Vec::new();
                }

                stdout
                    .lines()
                    .filter_map(|line| {
                        let line = line.trim().trim_matches('\'');
                        if line.is_empty() || line == "no-tmux" {
                            return None;
                        }
                        let parts = line.split("|||").collect::<Vec<_>>();
                        if parts.len() < 3 {
                            return None;
                        }

                        Some(TmuxSessionInfo {
                            session_name: parts[0].to_string(),
                            vm_name: vm_name.to_string(),
                            windows: parts[1].parse::<u32>().unwrap_or(1),
                            attached: parts[2] == "1",
                            agent_status: AgentStatus::Unknown,
                            last_output: String::new(),
                            working_directory: String::new(),
                            repo_url: String::new(),
                            git_branch: String::new(),
                            pr_url: String::new(),
                            task_summary: String::new(),
                        })
                    })
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(unix)]
pub(super) fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(super) fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

// ---------------------------------------------------------------------------
// Issue #871 — a failed `azlin list` (timeout, non-zero exit, missing binary)
// must NOT be swallowed into an empty VM list treated as "no VMs" (which drives
// double-provisioning and orphaned VMs).
//
// `parse_vm_json`'s contract is intentionally unchanged (empty on parse error,
// so the caller falls back to the text parser). The observable fix is in
// `poll_vms`: the json->text fallback transition is logged at warn!, and a
// terminal empty (both attempts failed) is logged at error!.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod issue_871_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    #[derive(Default)]
    struct CaptureSubscriber {
        lines: Arc<Mutex<Vec<String>>>,
        next_id: Arc<AtomicU64>,
    }

    impl Subscriber for CaptureSubscriber {
        fn enabled(&self, meta: &Metadata<'_>) -> bool {
            *meta.level() <= tracing::Level::WARN
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut grabber = FieldGrabber::default();
            event.record(&mut grabber);
            let mut line = event.metadata().level().to_string();
            line.push(' ');
            line.push_str(&grabber.fields.join(" "));
            self.lines
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(line);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
        fn register_callsite(
            &self,
            _: &'static Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::always()
        }
        fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
            Some(tracing::metadata::LevelFilter::WARN)
        }
    }

    #[derive(Default)]
    struct FieldGrabber {
        fields: Vec<String>,
    }
    impl Visit for FieldGrabber {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }

    fn capture<T>(op: impl FnOnce() -> T) -> (T, Vec<String>) {
        let sub = CaptureSubscriber::default();
        let lines = Arc::clone(&sub.lines);
        let out = tracing::subscriber::with_default(sub, op);
        let captured = lines.lock().unwrap_or_else(|p| p.into_inner()).clone();
        (out, captured)
    }

    fn has_level(lines: &[String], level: &str) -> bool {
        lines.iter().any(|l| l.starts_with(level))
    }

    #[test]
    fn parse_vm_json_valid_returns_vms() {
        let vms =
            FleetState::parse_vm_json(r#"[{"name":"vm-a","status":"running","region":"eastus"}]"#);
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].name, "vm-a");
    }

    #[test]
    fn parse_vm_json_malformed_returns_empty() {
        // Contract intentionally unchanged: empty drives the text fallback.
        assert!(FleetState::parse_vm_json("}{ not json").is_empty());
    }

    #[test]
    fn poll_vms_terminal_failure_is_logged() {
        // A missing azlin binary makes both the json and text attempts fail.
        let state = FleetState::new(PathBuf::from("/nonexistent/azlin-binary-xyzzy"));
        let (vms, logs) = capture(|| state.poll_vms());
        assert!(vms.is_empty());
        assert!(
            has_level(&logs, "ERROR"),
            "a failed azlin poll must emit error! and never be treated as an \
             empty fleet; got: {logs:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn poll_vms_json_failure_falls_back_to_text_with_warning() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        // `list --json` exits non-zero; `list` (text) succeeds with one VM row.
        fs::write(
            &azlin,
            "#!/bin/sh\nif [ \"$2\" = \"--json\" ]; then echo boom 1>&2; exit 1; fi\nprintf 'Name Session Tmux Status\\nvmx │ vmx │ linux │ running │ 10.0.0.1 │ eastus\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&azlin, fs::Permissions::from_mode(0o755)).unwrap();

        let state = FleetState::new(azlin);
        let (vms, logs) = capture(|| state.poll_vms());
        assert_eq!(vms.len(), 1, "the text fallback must recover the inventory");
        assert!(
            has_level(&logs, "WARN"),
            "the json->text fallback transition must be logged at warn!; got: {logs:?}"
        );
        assert!(
            !has_level(&logs, "ERROR"),
            "a successful text fallback is not a terminal failure; got: {logs:?}"
        );
    }
}
