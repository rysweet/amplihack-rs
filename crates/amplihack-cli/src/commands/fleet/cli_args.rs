use super::*;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct NativeFleetCli {
    #[command(subcommand)]
    pub(crate) command: NativeFleetCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NativeFleetCommand {
    Setup,
    Status,
    Snapshot,
    Tui {
        #[arg(long, default_value_t = DEFAULT_DASHBOARD_REFRESH_SECONDS)]
        interval: u64,
        #[arg(long = "capture-lines", default_value_t = DEFAULT_CAPTURE_LINES)]
        capture_lines: usize,
    },
    DryRun {
        #[arg(long = "vm")]
        vm: Vec<String>,
        #[arg(long, default_value = "")]
        priorities: String,
        #[arg(long, default_value = "auto")]
        backend: String,
    },
    Scout {
        #[arg(long)]
        vm: Option<String>,
        #[arg(long = "session")]
        session_target: Option<String>,
        #[arg(long = "skip-adopt")]
        skip_adopt: bool,
        #[arg(long)]
        incremental: bool,
        #[arg(long = "save")]
        save_path: Option<PathBuf>,
    },
    Advance {
        #[arg(long)]
        vm: Option<String>,
        #[arg(long = "session")]
        session_target: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long = "save")]
        save_path: Option<PathBuf>,
    },
    Start {
        #[arg(long = "max-cycles", default_value_t = 0)]
        max_cycles: u32,
        #[arg(long, default_value_t = DEFAULT_POLL_INTERVAL_SECONDS)]
        interval: u64,
        #[arg(long)]
        adopt: bool,
        #[arg(long = "stuck-threshold", default_value_t = DEFAULT_STUCK_THRESHOLD_SECONDS)]
        stuck_threshold: f64,
        #[arg(long = "max-agents-per-vm", default_value_t = DEFAULT_MAX_AGENTS_PER_VM)]
        max_agents_per_vm: usize,
        #[arg(long = "capture-lines", default_value_t = DEFAULT_CAPTURE_LINES)]
        capture_lines: usize,
    },
    RunOnce,
    Auth {
        vm_name: String,
        #[arg(long = "services", default_values = ["github", "azure", "claude"])]
        services: Vec<String>,
    },
    Adopt {
        vm_name: String,
        #[arg(long = "sessions")]
        sessions: Vec<String>,
    },
    Observe {
        vm_name: String,
    },
    Report,
    Queue,
    Dashboard,
    Graph,
    CopilotStatus,
    CopilotLog {
        #[arg(long, default_value_t = 0)]
        tail: usize,
    },
    Project {
        #[command(subcommand)]
        command: NativeFleetProjectCommand,
    },
    Watch {
        vm_name: String,
        session_name: String,
        #[arg(long, default_value_t = 30)]
        lines: u32,
    },
    AddTask {
        prompt: String,
        #[arg(long, default_value = "")]
        repo: String,
        #[arg(long, value_enum, default_value_t = NativeTaskPriorityArg::Medium)]
        priority: NativeTaskPriorityArg,
        #[arg(long, value_enum, default_value_t = NativeAgentArg::Claude)]
        agent: NativeAgentArg,
        #[arg(long, value_enum, default_value_t = NativeAgentModeArg::Auto)]
        mode: NativeAgentModeArg,
        #[arg(long = "max-turns", default_value_t = DEFAULT_MAX_TURNS)]
        max_turns: u32,
        #[arg(long)]
        protected: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum NativeFleetProjectCommand {
    Add {
        repo_url: String,
        #[arg(long = "identity", default_value = "")]
        identity: String,
        #[arg(long, value_enum, default_value_t = NativeProjectPriorityArg::Medium)]
        priority: NativeProjectPriorityArg,
        #[arg(long, default_value = "")]
        name: String,
    },
    List,
    Remove {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum NativeProjectPriorityArg {
    Low,
    Medium,
    High,
}

impl NativeProjectPriorityArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NativeProjectPriorityArg::Low => "low",
            NativeProjectPriorityArg::Medium => "medium",
            NativeProjectPriorityArg::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum NativeTaskPriorityArg {
    Critical,
    High,
    Medium,
    Low,
}

impl NativeTaskPriorityArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NativeTaskPriorityArg::Critical => "critical",
            NativeTaskPriorityArg::High => "high",
            NativeTaskPriorityArg::Medium => "medium",
            NativeTaskPriorityArg::Low => "low",
        }
    }

    pub(crate) fn into_task_priority(self) -> TaskPriority {
        match self {
            NativeTaskPriorityArg::Critical => TaskPriority::Critical,
            NativeTaskPriorityArg::High => TaskPriority::High,
            NativeTaskPriorityArg::Medium => TaskPriority::Medium,
            NativeTaskPriorityArg::Low => TaskPriority::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum NativeAgentArg {
    Claude,
    Amplifier,
    Copilot,
}

impl NativeAgentArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NativeAgentArg::Claude => "claude",
            NativeAgentArg::Amplifier => "amplifier",
            NativeAgentArg::Copilot => "copilot",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum NativeAgentModeArg {
    Auto,
    Ultrathink,
}

impl NativeAgentModeArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NativeAgentModeArg::Auto => "auto",
            NativeAgentModeArg::Ultrathink => "ultrathink",
        }
    }
}
