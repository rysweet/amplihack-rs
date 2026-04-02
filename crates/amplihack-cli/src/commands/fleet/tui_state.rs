use super::*;

pub(super) enum FleetTuiRow<'a> {
    Session(&'a VmInfo, &'a TmuxSessionInfo),
    Placeholder(&'a VmInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum FleetSubview {
    #[default]
    Managed,
    AllSessions,
}

impl FleetSubview {
    pub(super) fn label(self) -> &'static str {
        match self {
            FleetSubview::Managed => "managed",
            FleetSubview::AllSessions => "all",
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            FleetSubview::Managed => "Managed Sessions",
            FleetSubview::AllSessions => "All Sessions",
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            FleetSubview::Managed => FleetSubview::AllSessions,
            FleetSubview::AllSessions => FleetSubview::Managed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum FleetTuiTab {
    #[default]
    Fleet,
    Detail,
    Projects,
    Editor,
    NewSession,
}

impl FleetTuiTab {
    pub(super) fn label(self) -> &'static str {
        match self {
            FleetTuiTab::Fleet => "fleet",
            FleetTuiTab::Detail => "detail",
            FleetTuiTab::Projects => "projects",
            FleetTuiTab::Editor => "editor",
            FleetTuiTab::NewSession => "new",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum FleetNewSessionAgent {
    #[default]
    Claude,
    Copilot,
    Amplifier,
}

impl FleetNewSessionAgent {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            FleetNewSessionAgent::Claude => "claude",
            FleetNewSessionAgent::Copilot => "copilot",
            FleetNewSessionAgent::Amplifier => "amplifier",
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            FleetNewSessionAgent::Claude => FleetNewSessionAgent::Copilot,
            FleetNewSessionAgent::Copilot => FleetNewSessionAgent::Amplifier,
            FleetNewSessionAgent::Amplifier => FleetNewSessionAgent::Claude,
        }
    }
}

/// Narrows the fleet view to sessions matching a particular status category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StatusFilter {
    /// Error or Stuck sessions only.
    Errors,
    /// Sessions awaiting user input.
    Waiting,
    /// Actively running or thinking sessions.
    Active,
}

impl StatusFilter {
    pub(super) fn label(self) -> &'static str {
        match self {
            StatusFilter::Errors => "errors",
            StatusFilter::Waiting => "waiting",
            StatusFilter::Active => "active",
        }
    }

    pub(super) fn matches(self, status: AgentStatus) -> bool {
        match self {
            StatusFilter::Errors => {
                matches!(status, AgentStatus::Error | AgentStatus::Stuck)
            }
            StatusFilter::Waiting => matches!(status, AgentStatus::WaitingInput),
            StatusFilter::Active => {
                matches!(status, AgentStatus::Running | AgentStatus::Thinking)
            }
        }
    }
}

/// T4: Commands sent from the render loop to background worker threads.
#[derive(Debug)]
pub(super) enum BackgroundCommand {
    /// Fast-status: re-poll fleet state (used by T2 adoption refresh trigger).
    ForceStatusRefresh,
    /// T1: Create a new tmux session on the given VM.
    CreateSession {
        azlin_path: PathBuf,
        vm_name: String,
        agent: String,
    },
}

/// T4: Messages sent back from background threads to the render loop.
#[derive(Debug)]
pub(super) enum BackgroundMessage {
    /// Fast background status refresh completed.
    FastStatusUpdate(FleetState),
    /// Slow background capture refresh completed for one session.
    SlowCaptureUpdate {
        vm_name: String,
        session_name: String,
        output: String,
    },
    /// T1: Session creation completed.
    SessionCreated { message: String },
    /// Any background error worth surfacing.
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FleetTuiSelection {
    pub(super) vm_name: String,
    pub(super) session_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FleetDetailCapture {
    pub(super) vm_name: String,
    pub(super) session_name: String,
    pub(super) output: String,
}

/// T6: Sub-modes for the Projects tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ProjectManagementMode {
    /// Viewing the project list.
    #[default]
    List,
    /// Adding a new project (inline input active).
    Add,
    /// Confirming removal of selected project.
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FleetProposalNotice {
    pub(super) vm_name: String,
    pub(super) session_name: String,
    pub(super) title: String,
    pub(super) message: String,
}

#[derive(Debug, Clone)]
pub(super) struct FleetTuiUiState {
    pub(super) tab: FleetTuiTab,
    pub(super) fleet_subview: FleetSubview,
    pub(super) selected: Option<FleetTuiSelection>,
    pub(super) selected_project_repo: Option<String>,
    /// T5: Per-session tmux capture cache (LRU, 64 entries).
    /// Key: (vm_name, session_name), Value: FleetDetailCapture.
    /// Wrapped in Arc<Mutex<>> so it is Clone and shareable with background threads.
    pub(super) capture_cache: Arc<Mutex<LruCache<(String, String), FleetDetailCapture>>>,
    /// Kept for backward compatibility — tests may set this; it is promoted into
    /// capture_cache on first access via `detail_capture_output()`.
    pub(super) detail_capture: Option<FleetDetailCapture>,
    pub(super) proposal_notice: Option<FleetProposalNotice>,
    pub(super) last_decision: Option<SessionDecision>,
    pub(super) editor_decision: Option<SessionDecision>,
    pub(super) new_session_vm: Option<String>,
    pub(super) new_session_agent: FleetNewSessionAgent,
    pub(super) status_message: Option<String>,
    pub(super) inline_input: Option<FleetTuiInlineInput>,
    pub(super) show_logo: bool,
    /// When true, the `?` help overlay is shown instead of the normal content.
    pub(super) show_help: bool,
    /// Optional filter applied to the fleet view (shows only matching sessions).
    pub(super) status_filter: Option<StatusFilter>,
    /// Optional case-insensitive search applied to VM/session names in the fleet view.
    pub(super) session_search: Option<String>,
    /// Temporary progress indicator while refresh is still enriching running VMs.
    pub(super) refresh_progress: Option<FleetRefreshProgress>,
    /// T4: Channel for sending commands to the background refresh worker.
    /// None when background threads are not running (tests, non-interactive mode).
    pub(super) bg_tx: Option<mpsc::Sender<BackgroundCommand>>,
    /// T4: Channel for receiving messages from background threads.
    pub(super) bg_rx: Option<Arc<Mutex<mpsc::Receiver<BackgroundMessage>>>>,
    /// T6: Current project management sub-mode.
    pub(super) project_mode: ProjectManagementMode,
    /// T1: Whether a session create is in-flight.
    pub(super) create_session_pending: bool,
    /// T3: Multiline proposal editor buffer (lines, cursor row).
    pub(super) editor_lines: Vec<String>,
    /// T3: Current cursor row in the multiline editor.
    pub(super) editor_cursor_row: usize,
    /// T3: Whether the multiline editor is active.
    pub(super) editor_active: bool,
}

impl Default for FleetTuiUiState {
    fn default() -> Self {
        Self {
            tab: FleetTuiTab::default(),
            fleet_subview: FleetSubview::default(),
            selected: None,
            selected_project_repo: None,
            capture_cache: Arc::new(Mutex::new(LruCache::new(CAPTURE_CACHE_CAPACITY_NONZERO))),
            detail_capture: None,
            proposal_notice: None,
            last_decision: None,
            editor_decision: None,
            new_session_vm: None,
            new_session_agent: FleetNewSessionAgent::default(),
            status_message: None,
            inline_input: None,
            show_logo: true,
            show_help: false,
            status_filter: None,
            session_search: None,
            refresh_progress: None,
            bg_tx: None,
            bg_rx: None,
            project_mode: ProjectManagementMode::default(),
            create_session_pending: false,
            editor_lines: Vec::new(),
            editor_cursor_row: 0,
            editor_active: false,
        }
    }
}
