use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FleetTuiInlineInputMode {
    AddProjectRepo,
    SearchSessions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FleetTuiInlineInput {
    pub(super) mode: FleetTuiInlineInputMode,
    pub(super) buffer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FleetRefreshProgress {
    pub(super) completed_vms: usize,
    pub(super) total_vms: usize,
    pub(super) current_vm: Option<String>,
}

impl FleetRefreshProgress {
    pub(super) fn label(&self) -> String {
        match (self.total_vms, self.current_vm.as_deref()) {
            (0, _) => "0/0".to_string(),
            (_, Some(vm_name)) => {
                format!(
                    "{}/{} polling {}",
                    self.completed_vms, self.total_vms, vm_name
                )
            }
            _ => format!("{}/{} complete", self.completed_vms, self.total_vms),
        }
    }
}

/// Compute aggregate session status counts for the fleet header.
///
/// Returns `(total, active, waiting, errors, idle)`.
pub(super) fn fleet_status_summary(
    state: &FleetState,
    subview: FleetSubview,
) -> (usize, usize, usize, usize, usize) {
    let mut total = 0usize;
    let mut active = 0usize;
    let mut waiting = 0usize;
    let mut errors = 0usize;
    let mut idle = 0usize;
    for vm in FleetTuiUiState::fleet_vms(state, subview)
        .into_iter()
        .filter(|vm| vm.is_running())
    {
        for session in &vm.tmux_sessions {
            total += 1;
            match session.agent_status {
                AgentStatus::Running | AgentStatus::Thinking => active += 1,
                AgentStatus::WaitingInput => waiting += 1,
                AgentStatus::Error | AgentStatus::Stuck => errors += 1,
                _ => idle += 1,
            }
        }
    }
    (total, active, waiting, errors, idle)
}

/// Returns the display sort priority for a status (lower = shown first).
pub(super) fn status_sort_priority(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Error => 0,
        AgentStatus::Stuck => 1,
        AgentStatus::WaitingInput => 2,
        AgentStatus::Running => 3,
        AgentStatus::Thinking => 4,
        AgentStatus::Idle => 5,
        AgentStatus::Shell => 6,
        AgentStatus::Completed => 7,
        AgentStatus::NoSession => 8,
        AgentStatus::Unreachable => 9,
        AgentStatus::Unknown => 10,
    }
}

#[cfg(unix)]
pub(super) struct DashboardTerminalGuard {
    pub(super) fd: i32,
    pub(super) original: Option<libc::termios>,
}

#[cfg(unix)]
impl DashboardTerminalGuard {
    pub(super) fn activate() -> Result<Self> {
        if !io::stdin().is_terminal() {
            return Ok(Self {
                fd: -1,
                original: None,
            });
        }

        let fd = io::stdin().as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            bail!("failed to read terminal attributes");
        }
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            bail!("failed to enable dashboard raw mode");
        }

        Ok(Self {
            fd,
            original: Some(original),
        })
    }
}

#[cfg(unix)]
impl Drop for DashboardTerminalGuard {
    fn drop(&mut self) {
        if let Some(original) = self.original.take() {
            let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &original) };
        }
    }
}

#[cfg(not(unix))]
pub(super) struct DashboardTerminalGuard;

#[cfg(not(unix))]
impl DashboardTerminalGuard {
    pub(super) fn activate() -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(not(unix))]
impl Drop for DashboardTerminalGuard {
    fn drop(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DashboardKey {
    Char(char),
    Left,
    Right,
    Up,
    Down,
}

pub(super) fn decode_dashboard_key_bytes(bytes: &[u8]) -> Option<DashboardKey> {
    match bytes {
        [byte] => Some(DashboardKey::Char(*byte as char)),
        [0x1b, b'[', b'A'] => Some(DashboardKey::Up),
        [0x1b, b'[', b'B'] => Some(DashboardKey::Down),
        [0x1b, b'[', b'C'] => Some(DashboardKey::Right),
        [0x1b, b'[', b'D'] => Some(DashboardKey::Left),
        _ => None,
    }
}

#[cfg(unix)]
pub(super) fn read_dashboard_key(timeout: Duration) -> Option<DashboardKey> {
    if !io::stdin().is_terminal() {
        thread::sleep(timeout);
        return None;
    }

    let fd = io::stdin().as_raw_fd();
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };

    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if ready <= 0 || (poll_fd.revents & libc::POLLIN) == 0 {
        return None;
    }

    let mut buffer = [0u8; 1];
    let bytes_read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), 1) };
    if bytes_read != 1 {
        return None;
    }
    if buffer[0] != 0x1b {
        return decode_dashboard_key_bytes(&buffer);
    }

    let mut bytes = vec![buffer[0]];
    for _ in 0..2 {
        let mut extra_poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut extra_poll, 1, 5) };
        if ready <= 0 || (extra_poll.revents & libc::POLLIN) == 0 {
            break;
        }
        let mut extra = [0u8; 1];
        let extra_read = unsafe { libc::read(fd, extra.as_mut_ptr().cast(), 1) };
        if extra_read != 1 {
            break;
        }
        bytes.push(extra[0]);
    }

    decode_dashboard_key_bytes(&bytes).or(Some(DashboardKey::Char('\u{1b}')))
}

#[cfg(not(unix))]
pub(super) fn read_dashboard_key(timeout: Duration) -> Option<DashboardKey> {
    thread::sleep(timeout);
    None
}
