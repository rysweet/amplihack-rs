//! Native `fleet` subcommands.
//!
//! The Rust CLI now owns the live `amplihack fleet` runtime surface. This
//! module preserves the Python behavior where practical while keeping the
//! implementation explicit, testable, and fully native.

use crate::binary_finder::BinaryFinder;
use crate::command_error::exit_error;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::iter;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FLEET_EXISTING_VMS: &[&str] = &[];
const FLEET_EXISTING_VMS_ENV: &str = "AMPLIHACK_FLEET_EXISTING_VMS";
const AZLIN_VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const AZLIN_LIST_TIMEOUT: Duration = Duration::from_secs(60);
const TMUX_LIST_TIMEOUT: Duration = Duration::from_secs(30);
const CLI_WATCH_TIMEOUT: Duration = Duration::from_secs(60);
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_MAX_TURNS: u32 = 20;
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 60;
const DEFAULT_DASHBOARD_REFRESH_SECONDS: u64 = 30;
const DEFAULT_MAX_AGENTS_PER_VM: usize = 3;
const DEFAULT_CAPTURE_LINES: usize = 50;
const MAX_CAPTURE_LINES: usize = 10_000;
const DEFAULT_STUCK_THRESHOLD_SECONDS: f64 = 300.0;
const CONFIDENCE_COMPLETION: f64 = 0.9;
const CONFIDENCE_ERROR: f64 = 0.85;
const CONFIDENCE_THINKING: f64 = 1.0;
const CONFIDENCE_RUNNING: f64 = 0.8;

fn configured_existing_vms() -> Vec<String> {
    match env::var(FLEET_EXISTING_VMS_ENV) {
        Ok(raw) => raw
            .split([',', '\n', '\r', '\t', ' '])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Err(_) => FLEET_EXISTING_VMS
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
    }
}
const CONFIDENCE_IDLE: f64 = 0.7;
const CONFIDENCE_DEFAULT_RUNNING: f64 = 0.5;
const CONFIDENCE_UNKNOWN: f64 = 0.3;
const MIN_CONFIDENCE_SEND: f64 = 0.6;
const MIN_CONFIDENCE_RESTART: f64 = 0.8;
const MIN_SUBSTANTIAL_OUTPUT_LEN: usize = 50;
const SCOUT_REASONER_TIMEOUT: Duration = Duration::from_secs(180);
const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

// ANSI color/style codes for the fleet cockpit renderer.
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_CYAN: &str = "\x1b[36m";

// Unicode box-drawing characters for the fleet cockpit border.
const BOX_TL: char = '\u{2554}'; // ╔ top-left double
const BOX_TR: char = '\u{2557}'; // ╗ top-right double
const BOX_BL: char = '\u{255a}'; // ╚ bottom-left double
const BOX_BR: char = '\u{255d}'; // ╝ bottom-right double
const BOX_HL: char = '\u{2550}'; // ═ horizontal double
const BOX_VL: char = '\u{2551}'; // ║ vertical double
const BOX_ML: char = '\u{2560}'; // ╠ middle-left junction
const BOX_MR: char = '\u{2563}'; // ╣ middle-right junction
const BOX_DASH: char = '\u{2500}'; // ─ thin horizontal (VM section separator)

const SESSION_REASONER_SYSTEM_PROMPT: &str = r#"You are a Fleet Admiral managing coding agent sessions across multiple VMs.

For each session, analyze the terminal output and transcript to decide what to do.

Valid actions:
- send_input
- wait
- escalate
- mark_complete
- restart

Respond with JSON only:
{
  "action": "send_input|wait|escalate|mark_complete|restart",
  "input_text": "text to type (only for send_input)",
  "reasoning": "why you chose this action",
  "confidence": 0.0
}

Rules:
- If the session status is thinking, always choose wait.
- Do not interrupt active tool calls or active reasoning.
- Prefer wait or escalate over risky send_input decisions.
- Never approve destructive operations.
- If the agent appears done and the transcript/output supports it, mark_complete.
- If the agent is stuck or errored and recovery is justified, restart.
- If confidence is low, choose wait or escalate instead of send_input."#;

const COMPLETION_PATTERNS: &[&str] = &[
    r"PR.*created",
    r"pull request.*created",
    r"GOAL_STATUS:\s*ACHIEVED",
    r"Workflow Complete",
    r"All \d+ steps completed",
    r"pushed to.*branch",
];
const ERROR_PATTERNS: &[&str] = &[
    r"(?:^|\n)\s*(?:ERROR|FATAL|CRITICAL):",
    r"Traceback \(most recent",
    r"panic:",
    r"GOAL_STATUS:\s*NOT_ACHIEVED",
    r"Permission denied",
    r"Authentication failed",
];
const WAITING_PATTERNS: &[&str] = &[
    r"[?]\s*\[Y/n\]",
    r"[?]\s*\[y/N\]",
    r"\(yes/no\)",
    r"Press .* to continue",
    r"Do you want to",
    r"^Enter\s+\w+\s*:",
    r"waiting for.*input",
];
const RUNNING_PATTERNS: &[&str] = &[
    r"Step \d+",
    r"Task.*in_progress",
    r"Building",
    r"Implementing",
    r"Analyzing",
    r"Reading file",
    r"Writing file",
    r"Running tests",
    r"Creating.*commit",
];
const IDLE_PATTERNS: &[&str] = &[r"\$\s*$", r"azureuser@.*:\~.*\$", r"❯\s*$"];
const SAFE_INPUT_PATTERNS: &[&str] = &[
    r"^[yYnN]$",
    r"^(yes|no)$",
    r"^/[a-z]",
    r"^(exit|quit|q)$",
    r"^\d+$",
    r"^(git status|git log|git diff|git branch)",
    r"^(ls|pwd|wc|which)\b",
    r"^(pytest|make|npm test|npm run|cargo test)",
];
const DANGEROUS_INPUT_PATTERNS: &[&str] = &[
    r"\brm\s+-rf\b",
    r"\brm\s+-r\s+/",
    r"\brmdir\s+/",
    r"\bshred\b",
    r">\s*/dev/sd[a-z]",
    r"\bmkfs\.",
    r"\bdd\s+if=",
    r"\bgit\s+push\s+--force\b",
    r"\bgit\s+push\s+-f\b",
    r"\bgit\s+reset\s+--hard\b",
    r"\bgit\s+clean\s+-fd",
    r"\bDROP\s+TABLE\b",
    r"\bDROP\s+DATABASE\b",
    r"\bDELETE\s+FROM\b",
    r"\bTRUNCATE\s+TABLE\b",
    r"\bcurl\b.*\|\s*\b(ba)?sh\b",
    r"\bwget\b.*\|\s*\b(ba)?sh\b",
    r"\bcurl\b.*-o\s*-\s*\|",
    r"\bwget\b.*-O\s*-\s*\|",
    r"\bpython[23]?\s+-c\b",
    r"\bperl\s+-e\b",
    r"\bruby\s+-e\b",
    r"\bnode\s+-e\b",
    r"\beval\s*\(",
    r"\bexec\s*\(",
    r"\bnc\s+-[elp]",
    r"\bncat\b.*-e",
    r"\bsocat\b",
    r"bash\s+-i\s+>&\s*/dev/tcp",
    r"/dev/tcp/",
    r"\bsudo\b",
    r"\bchmod\s+\+s\b",
    r"\bchmod\s+777\b",
    r"\bchown\s+root\b",
    r"\bcat\s+.*/etc/shadow\b",
    r"\bcat\s+.*\.ssh/id_",
    r"\bcat\s+.*\.claude\.json\b",
    r"\bcat\s+.*/hosts\.yml\b",
    r"\bprintenv\b",
    r"\benv\s*$",
    r"\bset\s*$",
    r"ANTHROPIC_API_KEY",
    r"GITHUB_TOKEN",
    r"AZURE_.*SECRET",
    r"\bcrontab\b",
    r"\bat\s+-f\b",
    r"\bsystemctl\s+enable\b",
    r">\s*~/\.bashrc\b",
    r">\s*~/\.profile\b",
    r">\s*/etc/",
    r"\bscp\b.*@",
    r"\brsync\b.*@",
    r"\bbase64\b.*\|.*\bcurl\b",
    r":\(\)\s*\{",
    r"\bfork\s*\(\)",
];
const AUTH_GITHUB_FILES: &[(&str, &str, &str)] = &[
    ("~/.config/gh/hosts.yml", "~/.config/gh/hosts.yml", "600"),
    ("~/.config/gh/config.yml", "~/.config/gh/config.yml", "600"),
];
const AUTH_AZURE_FILES: &[(&str, &str, &str)] = &[
    (
        "~/.azure/msal_token_cache.json",
        "~/.azure/msal_token_cache.json",
        "600",
    ),
    (
        "~/.azure/azureProfile.json",
        "~/.azure/azureProfile.json",
        "644",
    ),
    ("~/.azure/clouds.config", "~/.azure/clouds.config", "644"),
];
const AUTH_CLAUDE_FILES: &[(&str, &str, &str)] = &[("~/.claude.json", "~/.claude.json", "600")];

pub fn run_fleet(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return run_tui(DEFAULT_DASHBOARD_REFRESH_SECONDS, DEFAULT_CAPTURE_LINES);
    }

    match parse_native_fleet_command(&args) {
        Some(NativeFleetCommand::Setup) => run_setup(),
        Some(NativeFleetCommand::Status) => run_status(),
        Some(NativeFleetCommand::Snapshot) => run_snapshot(),
        Some(NativeFleetCommand::Tui {
            interval,
            capture_lines,
        }) => run_tui(interval, capture_lines),
        Some(NativeFleetCommand::DryRun {
            vm,
            priorities,
            backend,
        }) => run_dry_run(&vm, &priorities, &backend),
        Some(NativeFleetCommand::Scout {
            vm,
            session_target,
            skip_adopt,
            incremental,
            save_path,
        }) => run_scout(
            vm.as_deref(),
            session_target.as_deref(),
            skip_adopt,
            incremental,
            save_path.as_deref(),
        ),
        Some(NativeFleetCommand::Advance {
            vm,
            session_target,
            force,
            save_path,
        }) => run_advance(
            vm.as_deref(),
            session_target.as_deref(),
            force,
            save_path.as_deref(),
        ),
        Some(NativeFleetCommand::Start {
            max_cycles,
            interval,
            adopt,
            stuck_threshold,
            max_agents_per_vm,
            capture_lines,
        }) => run_start(
            max_cycles,
            interval,
            adopt,
            stuck_threshold,
            max_agents_per_vm,
            capture_lines,
        ),
        Some(NativeFleetCommand::RunOnce) => run_run_once(),
        Some(NativeFleetCommand::Auth { vm_name, services }) => run_auth(&vm_name, &services),
        Some(NativeFleetCommand::Adopt { vm_name, sessions }) => run_adopt(&vm_name, &sessions),
        Some(NativeFleetCommand::Observe { vm_name }) => run_observe(&vm_name),
        Some(NativeFleetCommand::Report) => run_report(),
        Some(NativeFleetCommand::Queue) => run_queue(),
        Some(NativeFleetCommand::Dashboard) => run_dashboard(),
        Some(NativeFleetCommand::Graph) => run_graph(),
        Some(NativeFleetCommand::CopilotStatus) => run_copilot_status(),
        Some(NativeFleetCommand::CopilotLog { tail }) => run_copilot_log(tail),
        Some(NativeFleetCommand::Project { command }) => run_project(command),
        Some(NativeFleetCommand::Watch {
            vm_name,
            session_name,
            lines,
        }) => run_watch(&vm_name, &session_name, lines),
        Some(NativeFleetCommand::AddTask {
            prompt,
            repo,
            priority,
            agent,
            mode,
            max_turns,
            protected,
        }) => run_add_task(&prompt, &repo, priority, agent, mode, max_turns, protected),
        None if args.iter().any(|arg| arg == "--help" || arg == "-h") => {
            let mut command = NativeFleetCli::command();
            command.print_help()?;
            println!();
            Ok(())
        }
        None => bail!("unsupported or invalid `amplihack fleet` subcommand"),
    }
}

fn parse_native_fleet_command(args: &[String]) -> Option<NativeFleetCommand> {
    let argv = iter::once("fleet").chain(args.iter().map(String::as_str));
    NativeFleetCli::try_parse_from(argv)
        .ok()
        .map(|cli| cli.command)
}

#[derive(Debug, Parser)]
struct NativeFleetCli {
    #[command(subcommand)]
    command: NativeFleetCommand,
}

#[derive(Debug, Subcommand)]
enum NativeFleetCommand {
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
enum NativeFleetProjectCommand {
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
enum NativeProjectPriorityArg {
    Low,
    Medium,
    High,
}

impl NativeProjectPriorityArg {
    fn as_str(self) -> &'static str {
        match self {
            NativeProjectPriorityArg::Low => "low",
            NativeProjectPriorityArg::Medium => "medium",
            NativeProjectPriorityArg::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum NativeTaskPriorityArg {
    Critical,
    High,
    Medium,
    Low,
}

impl NativeTaskPriorityArg {
    fn as_str(self) -> &'static str {
        match self {
            NativeTaskPriorityArg::Critical => "critical",
            NativeTaskPriorityArg::High => "high",
            NativeTaskPriorityArg::Medium => "medium",
            NativeTaskPriorityArg::Low => "low",
        }
    }

    fn into_task_priority(self) -> TaskPriority {
        match self {
            NativeTaskPriorityArg::Critical => TaskPriority::Critical,
            NativeTaskPriorityArg::High => TaskPriority::High,
            NativeTaskPriorityArg::Medium => TaskPriority::Medium,
            NativeTaskPriorityArg::Low => TaskPriority::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum NativeAgentArg {
    Claude,
    Amplifier,
    Copilot,
}

impl NativeAgentArg {
    fn as_str(self) -> &'static str {
        match self {
            NativeAgentArg::Claude => "claude",
            NativeAgentArg::Amplifier => "amplifier",
            NativeAgentArg::Copilot => "copilot",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum NativeAgentModeArg {
    Auto,
    Ultrathink,
}

impl NativeAgentModeArg {
    fn as_str(self) -> &'static str {
        match self {
            NativeAgentModeArg::Auto => "auto",
            NativeAgentModeArg::Ultrathink => "ultrathink",
        }
    }
}

fn run_setup() -> Result<()> {
    println!("Fleet setup — checking prerequisites...");
    let mut all_ok = true;

    let azlin_path = match get_azlin_path() {
        Ok(path) => {
            println!("  azlin: {}", path.display());
            Some(path)
        }
        Err(_) => {
            eprintln!("  azlin: NOT FOUND");
            eprintln!("    Install azlin and set AZLIN_PATH, or add it to PATH.");
            eprintln!("    See: https://github.com/rysweet/azlin");
            all_ok = false;
            None
        }
    };

    if let Some(path) = azlin_path {
        let mut version_cmd = Command::new(&path);
        version_cmd.arg("--version");
        match run_output_with_timeout(version_cmd, AZLIN_VERSION_TIMEOUT) {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout);
                let version = version.trim();
                let version = if version.is_empty() {
                    "unknown"
                } else {
                    version
                };
                println!("  azlin version: {version}");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("  azlin: found but --version failed ({})", stderr.trim());
            }
            Err(err) => {
                eprintln!("  azlin: found but verification failed — {err}");
            }
        }
    }

    if let Some(path) = find_binary("az") {
        println!("  az CLI: {}", path.display());
    } else {
        println!("  az CLI: not found (optional — needed for VM provisioning)");
    }

    if all_ok {
        println!();
        println!("All prerequisites found.");
        Ok(())
    } else {
        eprintln!();
        eprintln!("Missing prerequisites — see errors above.");
        Err(exit_error(1))
    }
}

fn run_status() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let mut state = FleetState::new(azlin);
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh();
    println!("{}", state.summary());
    Ok(())
}

fn run_snapshot() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let mut state = FleetState::new(azlin.clone());
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh();

    let mut observer = FleetObserver::new(azlin);
    println!("{}", render_snapshot(&state, &mut observer)?);
    Ok(())
}

fn run_dry_run(vm_names: &[String], priorities: &str, backend: &str) -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let sessions = discover_dry_run_sessions(&azlin, vm_names)?;
    if sessions.is_empty() {
        return Ok(());
    }

    let backend = NativeReasonerBackend::detect(backend)?;
    let mut reasoner = FleetSessionReasoner::new(azlin, backend);
    println!();
    println!("Fleet Admiral Dry Run -- {} sessions", sessions.len());
    println!("Backend: {}", reasoner.backend_label());
    println!(
        "Priorities: {}",
        if priorities.is_empty() {
            "(none specified)"
        } else {
            priorities
        }
    );
    println!();

    for session in sessions {
        let _ = reasoner.reason_about_session(
            &session.vm_name,
            &session.session_name,
            "",
            priorities,
            None,
        )?;
    }

    println!("\n{}", reasoner.dry_run_report());
    Ok(())
}

fn run_scout(
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
        let mut queue = TaskQueue::load(Some(default_queue_path()))?;
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
            match load_previous_scout(&last_scout_path) {
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
            println!(
                "  Skipping (unchanged): {} [{}]",
                session_key, session_status
            );
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
    snapshot.save(&default_last_scout_path())?;
    if let Some(path) = save_path {
        snapshot.save(path)?;
        println!("\nJSON report saved to: {}", path.display());
    }
    Ok(())
}

fn run_advance(
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

fn confirm_action(prompt: &str, default: bool) -> Result<bool> {
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

fn default_last_scout_path() -> PathBuf {
    fleet_home_dir().join("last_scout.json")
}

fn run_start(
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

    let queue = TaskQueue::load(Some(default_queue_path()))?;
    let mut admiral = FleetAdmiral::new(azlin, queue, Some(default_log_dir()))?;
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    admiral.exclude_vms(&existing_refs);
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

fn run_run_once() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let queue = TaskQueue::load(Some(default_queue_path()))?;
    let mut admiral = FleetAdmiral::new(azlin, queue, Some(default_log_dir()))?;
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    admiral.exclude_vms(&existing_refs);

    let actions = admiral.run_once()?;
    println!("Cycle completed: {} actions taken", actions.len());
    for action in actions {
        println!("  {}: {}", action.action_type.as_str(), action.reason);
    }
    Ok(())
}

fn run_auth(vm_name: &str, services: &[String]) -> Result<()> {
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

fn run_adopt(vm_name: &str, sessions: &[String]) -> Result<()> {
    validate_vm_name(vm_name)?;
    for session in sessions {
        validate_session_name(session)?;
    }
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let adopter = SessionAdopter::new(azlin);
    let mut queue = TaskQueue::load(Some(default_queue_path()))?;

    println!("Discovering sessions on {vm_name}...");
    let discovered = adopter.discover_sessions(vm_name);
    if discovered.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("Found {} sessions:", discovered.len());
    for session in &discovered {
        println!("  {}", session.session_name);
        if !session.inferred_repo.is_empty() {
            println!("    Repo: {}", session.inferred_repo);
        }
        if !session.inferred_branch.is_empty() {
            println!("    Branch: {}", session.inferred_branch);
        }
        if !session.agent_type.is_empty() {
            println!("    Agent: {}", session.agent_type);
        }
    }

    let session_filter = (!sessions.is_empty()).then(|| sessions.to_vec());
    let adopted = adopter.adopt_sessions(vm_name, &mut queue, session_filter.as_deref())?;

    println!("\nAdopted {} sessions:", adopted.len());
    for session in &adopted {
        if let Some(task_id) = &session.task_id {
            println!("  {} -> task {}", session.session_name, task_id);
        }
    }

    Ok(())
}

fn run_report() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let queue = TaskQueue::load(Some(default_queue_path()))?;
    let state = perceive_fleet_state(azlin)?;
    println!("{}", render_report(&state, &queue));
    Ok(())
}

fn run_observe(vm_name: &str) -> Result<()> {
    validate_vm_name(vm_name)?;
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let mut state = FleetState::new(azlin.clone());
    state.refresh();

    let Some(vm) = state.get_vm(vm_name) else {
        println!("VM not found: {vm_name}");
        return Err(exit_error(1));
    };

    if vm.tmux_sessions.is_empty() {
        println!("No tmux sessions on {vm_name}");
        return Ok(());
    }

    let observer = FleetObserver::new(azlin);
    println!("{}", render_observe(vm, &observer)?);
    Ok(())
}

fn run_queue() -> Result<()> {
    let queue = TaskQueue::load(Some(default_queue_path()))?;
    println!("{}", queue.summary());
    Ok(())
}

fn run_add_task(
    prompt: &str,
    repo: &str,
    priority: NativeTaskPriorityArg,
    agent: NativeAgentArg,
    mode: NativeAgentModeArg,
    max_turns: u32,
    protected: bool,
) -> Result<()> {
    let _ = protected;
    let mut queue = TaskQueue::load(Some(default_queue_path()))?;
    let task = queue.add_task(
        prompt,
        repo,
        priority.into_task_priority(),
        agent.as_str(),
        mode.as_str(),
        max_turns,
    )?;

    println!("Task {} added: {}", task.id, truncate_chars(prompt, 80));
    println!(
        "Priority: {}, Agent: {}, Mode: {}",
        priority.as_str(),
        agent.as_str(),
        mode.as_str()
    );
    Ok(())
}

fn run_graph() -> Result<()> {
    let graph = FleetGraphSummary::load(Some(default_graph_path()))?;
    println!("{}", graph.summary());
    Ok(())
}

fn run_dashboard() -> Result<()> {
    let queue = TaskQueue::load(Some(default_queue_path()))?;
    let mut dashboard = FleetDashboardSummary::load(Some(default_dashboard_path()))?;
    dashboard.update_from_queue(&queue)?;
    println!("{}", dashboard.summary());
    Ok(())
}

fn run_project(command: NativeFleetProjectCommand) -> Result<()> {
    match command {
        NativeFleetProjectCommand::Add {
            repo_url,
            identity,
            priority,
            name,
        } => run_project_add(&repo_url, &identity, priority.as_str(), &name),
        NativeFleetProjectCommand::List => run_project_list(),
        NativeFleetProjectCommand::Remove { name } => run_project_remove(&name),
    }
}

fn run_project_add(repo_url: &str, identity: &str, priority: &str, name: &str) -> Result<()> {
    let mut dashboard = FleetDashboardSummary::load(Some(default_dashboard_path()))?;
    let existing = dashboard.get_project(repo_url).or_else(|| {
        (!name.is_empty())
            .then(|| dashboard.get_project(name))
            .flatten()
    });
    if let Some(existing) = existing {
        println!(
            "Project already registered: {} ({})",
            existing.name, existing.repo_url
        );
        return Ok(());
    }

    let index = dashboard.add_project(repo_url, identity, name, priority);
    dashboard.save()?;
    let project = dashboard.projects[index].clone();

    let mut projects = load_projects_registry(&default_projects_path())?;
    if !projects.contains_key(&project.name) {
        projects.insert(
            project.name.clone(),
            ProjectRegistryEntry {
                repo_url: repo_url.to_string(),
                identity: identity.to_string(),
                priority: priority.to_string(),
                objectives: Vec::new(),
            },
        );
        save_projects_registry(&projects, &default_projects_path())?;
    }

    println!("Added project: {}", project.name);
    println!("  Repo: {}", project.repo_url);
    if !identity.is_empty() {
        println!("  Identity: {identity}");
    }
    println!("  Priority: {priority}");
    Ok(())
}

fn run_project_list() -> Result<()> {
    let dashboard = FleetDashboardSummary::load(Some(default_dashboard_path()))?;
    println!("{}", render_project_list(&dashboard));
    Ok(())
}

fn run_project_remove(name: &str) -> Result<()> {
    let mut dashboard = FleetDashboardSummary::load(Some(default_dashboard_path()))?;
    if dashboard.remove_project(name) {
        dashboard.save()?;
        println!("Removed project: {name}");
    } else {
        println!("Project not found: {name}");
    }
    Ok(())
}

fn run_copilot_status() -> Result<()> {
    println!("{}", render_copilot_status(&default_copilot_lock_dir())?);
    Ok(())
}

fn run_copilot_log(tail: usize) -> Result<()> {
    let report = read_copilot_log(&default_copilot_log_dir(), tail)?;
    for _ in 0..report.malformed_entries {
        eprintln!("  (skipped malformed entry)");
    }
    println!("{}", report.rendered);
    Ok(())
}

fn run_watch(vm_name: &str, session_name: &str, lines: u32) -> Result<()> {
    run_watch_with_timeout(vm_name, session_name, lines, CLI_WATCH_TIMEOUT)
}

fn run_watch_with_timeout(
    vm_name: &str,
    session_name: &str,
    lines: u32,
    timeout: Duration,
) -> Result<()> {
    validate_vm_name(vm_name)?;
    validate_session_name(session_name)?;
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let lines = lines.clamp(1, 10_000);
    let command = format!("tmux capture-pane -t {session_name} -p -S -{lines}");
    let mut cmd = Command::new(azlin);
    cmd.args(["connect", vm_name, "--no-tmux", "--", &command]);

    match run_output_with_timeout(cmd, timeout) {
        Ok(output) if output.status.success() => {
            println!("--- {vm_name}/{session_name} ---");
            let stdout = String::from_utf8_lossy(&output.stdout);
            print!("{stdout}");
            if !stdout.ends_with('\n') {
                println!();
            }
            println!("--- end ---");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("Failed to capture: {}", truncate_chars(stderr.trim(), 200));
        }
        Err(error) if error.to_string().contains("timed out after") => {
            println!("Timeout connecting to VM");
        }
        Err(error) => return Err(error),
    }

    Ok(())
}

fn run_tui(interval: u64, capture_lines: usize) -> Result<()> {
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

    let terminal_guard = DashboardTerminalGuard::activate()?;
    let mut stdout = io::stdout();
    write!(stdout, "{HIDE_CURSOR}")?;
    stdout
        .flush()
        .context("failed to flush dashboard prelude")?;

    let result = (|| -> Result<()> {
        loop {
            let state = collect_observed_fleet_state(&azlin, capture_lines)?;
            ui_state.sync_to_state(&state);
            let frame = render_tui_frame(&state, interval, &ui_state)?;
            write!(stdout, "{CLEAR_SCREEN}{frame}")?;
            stdout.flush().context("failed to flush dashboard frame")?;

            match read_dashboard_key(Duration::from_secs(interval)) {
                // Quit
                Some(DashboardKey::Char('q')) | Some(DashboardKey::Char('Q')) => break,
                // Toggle help overlay
                Some(DashboardKey::Char('?')) => ui_state.show_help = !ui_state.show_help,
                Some(DashboardKey::Char('\u{1b}'))
                | Some(DashboardKey::Char('b'))
                | Some(DashboardKey::Char('B'))
                    if !ui_state.show_help =>
                {
                    ui_state.navigate_back();
                }
                // Force refresh — just fall through to re-collect state (next loop iter)
                Some(DashboardKey::Char('r')) | Some(DashboardKey::Char('R')) => continue,
                // Navigation
                Some(DashboardKey::Char('j'))
                | Some(DashboardKey::Char('J'))
                | Some(DashboardKey::Down) => ui_state.move_selection(&state, 1),
                Some(DashboardKey::Char('k'))
                | Some(DashboardKey::Char('K'))
                | Some(DashboardKey::Up) => ui_state.move_selection(&state, -1),
                // Tab cycling: Tab key = '\t' (forward), '[' = backward substitute.
                Some(DashboardKey::Char('\t')) | Some(DashboardKey::Right) => {
                    ui_state.cycle_tab_forward();
                }
                Some(DashboardKey::Char('[')) | Some(DashboardKey::Left) => {
                    ui_state.cycle_tab_backward();
                }
                // Direct tab jumps
                Some(DashboardKey::Char('1'))
                | Some(DashboardKey::Char('f'))
                | Some(DashboardKey::Char('F')) => ui_state.tab = FleetTuiTab::Fleet,
                Some(DashboardKey::Char('2'))
                | Some(DashboardKey::Char('s'))
                | Some(DashboardKey::Char('S')) => {
                    ui_state.tab = FleetTuiTab::Detail;
                }
                Some(DashboardKey::Char('\n')) | Some(DashboardKey::Char('\r'))
                    if ui_state.tab == FleetTuiTab::NewSession =>
                {
                    run_tui_create_session(&azlin, &mut ui_state)?;
                }
                Some(DashboardKey::Char('\n')) | Some(DashboardKey::Char('\r')) => {
                    ui_state.tab = FleetTuiTab::Detail;
                }
                Some(DashboardKey::Char('3'))
                | Some(DashboardKey::Char('p'))
                | Some(DashboardKey::Char('P')) => ui_state.tab = FleetTuiTab::Projects,
                Some(DashboardKey::Char('4')) => ui_state.tab = FleetTuiTab::Editor,
                Some(DashboardKey::Char('5'))
                | Some(DashboardKey::Char('n'))
                | Some(DashboardKey::Char('N')) => ui_state.tab = FleetTuiTab::NewSession,
                Some(DashboardKey::Char('e')) => run_tui_edit(&mut ui_state),
                Some(DashboardKey::Char('i')) | Some(DashboardKey::Char('I')) => {
                    run_tui_edit_input(&mut ui_state, &terminal_guard)?
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
                // Actions
                Some(DashboardKey::Char('d')) | Some(DashboardKey::Char('D')) => {
                    run_tui_dry_run(&azlin, &state, &mut ui_state)?;
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
        }
        Ok(())
    })();

    writeln!(stdout, "{SHOW_CURSOR}")?;
    stdout
        .flush()
        .context("failed to flush dashboard teardown")?;
    result
}

fn render_tui_once(azlin_path: &Path, interval: u64, capture_lines: usize) -> Result<String> {
    let state = collect_observed_fleet_state(azlin_path, capture_lines)?;
    let mut ui_state = FleetTuiUiState::default();
    ui_state.sync_to_state(&state);
    render_tui_frame(&state, interval, &ui_state)
}

/// Return the terminal width, capped at 100 columns for readability.
fn terminal_cols() -> usize {
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
fn cockpit_boxline(content: &str, visible_len: usize, inner: usize) -> String {
    let pad = inner.saturating_sub(visible_len);
    format!(
        "{ANSI_BOLD}{BOX_VL}{ANSI_RESET} {content}{}{ANSI_BOLD}{BOX_VL}{ANSI_RESET}",
        " ".repeat(pad),
    )
}

/// Return the ANSI color string and Unicode status icon for a session status.
fn status_color_and_icon(status: AgentStatus) -> (&'static str, &'static str) {
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

fn render_tui_frame(
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
        FleetTuiTab::Projects,
        FleetTuiTab::Editor,
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
        FleetTuiTab::Projects,
        FleetTuiTab::Editor,
        FleetTuiTab::NewSession,
    ]
    .iter()
    .map(|t| t.label().len() + 2)
    .sum::<usize>()
        + 4 * 3; // 4 " | " separators

    // ---------- status summary line ------------------------------------------
    let filter_hint = ui_state
        .status_filter
        .map(|f| format!("  [filter: {}]", f.label()))
        .unwrap_or_default();
    // Unicode status icons used inline (must be string literals in format! args).
    let icon_filled = "\u{25c9}"; // ◉
    let icon_circle = "\u{25cf}"; // ●
    let icon_cross = "\u{2717}"; // ✗
    let status_parts = format!(
        "{ANSI_GREEN}{icon_filled} active: {active}{ANSI_RESET}  \
         {ANSI_YELLOW}{icon_circle} idle: {idle}{ANSI_RESET}  \
         {ANSI_CYAN}{icon_filled} waiting: {waiting}{ANSI_RESET}  \
         {ANSI_RED}{icon_cross} error: {errors}{ANSI_RESET}{filter_hint}"
    );
    let status_raw =
        format!("active: {active}  idle: {idle}  waiting: {waiting}  error: {errors}{filter_hint}")
            .len()
            + 4 * 2; // icon + space per segment

    // ---------- controls line -------------------------------------------------
    let controls_text = format!(
        "  q quit  b back  r refresh  t view/action  d dry-run  a apply  A adopt/apply-edited  ? help  ({}s)",
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
            FleetTuiTab::Projects => cockpit_render_projects_view(&mut lines, inner)?,
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

// ── Cockpit boxed-content render helpers ─────────────────────────────────────

fn cockpit_render_help_overlay(lines: &mut Vec<String>, inner: usize) {
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
    push("  3 / p / P      Jump to Projects tab");
    push("  4              Jump to Editor tab");
    push("  5 / n / N      Jump to New Session tab");
    push("  Esc / b / B    Back: editor->detail, detail/projects->fleet");
    push("");
    push("Actions");
    push("  e              Load selected proposal into the editor");
    push("  i / I          Edit editor input text (inline prompt)");
    push("  t / T          Cycle fleet subview, editor action, or new-session agent type");
    push("  d / D          Dry-run reasoner on selected session");
    push("  a              Apply last prepared proposal to session");
    push("  A              Adopt selected session (fleet) or apply edited proposal (editor)");
    push("  x / X          Skip (discard) the current prepared proposal");
    push("  Enter          Open detail tab or create new session");
    push("");
    push("Filters — fleet view (press same key again to clear)");
    push("  E              Show only Error/Stuck sessions");
    push("  w / W          Show only WaitingInput sessions");
    push("  c / C          Show only Active (Running/Thinking) sessions");
    push("  * / 0          Clear all filters");
    push("");
    push("  ?              Toggle this help overlay");
}

fn cockpit_render_fleet_view(
    state: &FleetState,
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) {
    let mut rows: Vec<FleetTuiRow<'_>> = Vec::new();
    for vm in FleetTuiUiState::fleet_vms(state, ui_state.fleet_subview)
        .into_iter()
        .filter(|vm| vm.is_running())
    {
        if vm.tmux_sessions.is_empty() {
            if ui_state.status_filter.is_none() {
                rows.push(FleetTuiRow::Placeholder(vm));
            }
            continue;
        }
        rows.extend(
            vm.tmux_sessions
                .iter()
                .filter(|session| {
                    ui_state
                        .status_filter
                        .is_none_or(|f| f.matches(session.agent_status))
                })
                .map(|session| FleetTuiRow::Session(vm, session)),
        );
    }
    rows.sort_by_key(|row| match row {
        FleetTuiRow::Session(vm, session) => (
            status_sort_priority(session.agent_status),
            vm.name.as_str(),
            session.session_name.as_str(),
        ),
        FleetTuiRow::Placeholder(vm) => (
            status_sort_priority(AgentStatus::NoSession),
            vm.name.as_str(),
            "",
        ),
    });

    let filter_label = ui_state
        .status_filter
        .map(|f| format!(" [filter: {}]", f.label()))
        .unwrap_or_default();
    let subviews = [FleetSubview::Managed, FleetSubview::AllSessions]
        .iter()
        .map(|subview| {
            let label = subview.label();
            if *subview == ui_state.fleet_subview {
                format!("{ANSI_BOLD}{ANSI_CYAN}[{label}]{ANSI_RESET}")
            } else {
                format!(" {label} ")
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let subviews_raw = [FleetSubview::Managed, FleetSubview::AllSessions]
        .iter()
        .map(|subview| subview.label().len() + 2)
        .sum::<usize>()
        + 3;
    lines.push(cockpit_boxline(&subviews, subviews_raw, inner));
    let heading = format!("{}{}", ui_state.fleet_subview.title(), filter_label);
    lines.push(cockpit_boxline(&heading, heading.len(), inner));
    lines.push(cockpit_boxline("", 0, inner));

    if rows.is_empty() {
        let msg = if ui_state.status_filter.is_some() {
            "No sessions match the current filter.  Press '*' to clear."
        } else {
            "No running tmux session output available."
        };
        lines.push(cockpit_boxline(msg, msg.len(), inner));
        return;
    }

    if let Some((vm, session)) = ui_state.selected_session(state) {
        let selected_heading = format!(
            "Selected session: {}/{} ({})",
            vm.name,
            session.session_name,
            session.agent_status.as_str()
        );
        lines.push(cockpit_boxline(
            &selected_heading,
            selected_heading.len(),
            inner,
        ));
        for metadata in [
            (!session.git_branch.is_empty()).then(|| format!("  branch: {}", session.git_branch)),
            (!session.repo_url.is_empty()).then(|| format!("  repo: {}", session.repo_url)),
            (!session.working_directory.is_empty())
                .then(|| format!("  cwd: {}", session.working_directory)),
            (!session.pr_url.is_empty()).then(|| format!("  pr: {}", session.pr_url)),
            (!session.task_summary.is_empty()).then(|| format!("  task: {}", session.task_summary)),
        ]
        .into_iter()
        .flatten()
        {
            let line = truncate_chars(&metadata, inner.saturating_sub(2));
            lines.push(cockpit_boxline(&line, line.len(), inner));
        }
        let preview: Vec<&str> = session
            .last_output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if preview.is_empty() {
            let none = "  no output captured";
            lines.push(cockpit_boxline(none, none.len(), inner));
        } else {
            for line in preview
                .iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                let line = truncate_chars(line, inner.saturating_sub(4));
                let raw = 2 + line.len();
                lines.push(cockpit_boxline(&format!("  {line}"), raw, inner));
            }
        }
        lines.push(cockpit_boxline("", 0, inner));
    }

    let mut current_vm: Option<&str> = None;
    for row in &rows {
        match row {
            FleetTuiRow::Session(vm, session) => {
                // VM section header (emitted once per VM block).
                if current_vm != Some(vm.name.as_str()) {
                    current_vm = Some(vm.name.as_str());
                    let management_label = if ui_state.fleet_subview == FleetSubview::AllSessions {
                        if state.is_managed_vm(&vm.name) {
                            " managed"
                        } else {
                            " unmanaged"
                        }
                    } else {
                        ""
                    };
                    let dash_len = inner.saturating_sub(vm.name.len() + management_label.len() + 4);
                    let dashes: String = std::iter::repeat_n(BOX_DASH, dash_len).collect();
                    let vm_hdr_raw = 2 + vm.name.len() + management_label.len() + 1 + dash_len;
                    let vm_hdr = format!(
                        "  {ANSI_BOLD}[{name}]{ANSI_RESET}{management_label} {ANSI_DIM}{dashes}{ANSI_RESET}",
                        name = vm.name,
                    );
                    lines.push(cockpit_boxline(&vm_hdr, vm_hdr_raw, inner));
                }

                let selected = ui_state.selection_matches(&vm.name, &session.session_name);
                let (color, icon) = status_color_and_icon(session.agent_status);
                let marker = if selected { ">" } else { " " };
                let status_label = session.agent_status.as_str().to_uppercase();
                let name = if session.session_name.len() > 18 {
                    format!("{}...", &session.session_name[..15])
                } else {
                    session.session_name.clone()
                };
                let sess_raw = 4 + 1 + 1 + name.len() + 2 + status_label.len();
                let sess_line = format!(
                    "  {marker} {color}{icon}{ANSI_RESET} {ANSI_BOLD}{name}{ANSI_RESET}  \
                     {ANSI_DIM}{status_label}{ANSI_RESET}"
                );
                lines.push(cockpit_boxline(&sess_line, sess_raw, inner));

                // Last-output preview (up to 2 lines).
                let preview: Vec<&str> = session
                    .last_output
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                if preview.is_empty() {
                    let no_out = "    | no output captured";
                    lines.push(cockpit_boxline(no_out, no_out.len(), inner));
                } else {
                    for line in preview
                        .iter()
                        .rev()
                        .take(2)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        let t = truncate_chars(line, inner.saturating_sub(6));
                        let pl_raw = 6 + t.len();
                        let pl = format!("    {ANSI_DIM}| {t}{ANSI_RESET}");
                        lines.push(cockpit_boxline(&pl, pl_raw, inner));
                    }
                }
                lines.push(cockpit_boxline("", 0, inner));
            }
            FleetTuiRow::Placeholder(vm) => {
                let management_label = if ui_state.fleet_subview == FleetSubview::AllSessions {
                    if state.is_managed_vm(&vm.name) {
                        " managed"
                    } else {
                        " unmanaged"
                    }
                } else {
                    ""
                };
                let (color, icon) = status_color_and_icon(AgentStatus::NoSession);
                let text = format!(
                    "   {color}{icon}{ANSI_RESET} {ANSI_DIM}{name}/(no sessions){management_label} (empty){ANSI_RESET}",
                    name = vm.name,
                );
                let text_raw = 3 + 1 + 1 + vm.name.len() + 14 + management_label.len() + 7;
                lines.push(cockpit_boxline(&text, text_raw, inner));
                lines.push(cockpit_boxline("", 0, inner));
            }
        }
    }
}

fn cockpit_render_detail_view(
    state: &FleetState,
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) {
    let push = |lines: &mut Vec<String>, text: &str| {
        lines.push(cockpit_boxline(text, text.len(), inner));
    };

    let Some((vm, session)) = ui_state.selected_session(state) else {
        push(lines, "No session selected.");
        return;
    };

    let hdr = format!("Session Detail — {}/{}", vm.name, session.session_name);
    push(lines, &hdr);
    push(lines, "");
    push(
        lines,
        &format!("Status:   {}", session.agent_status.as_str()),
    );
    push(lines, &format!("Windows:  {}", session.windows));
    push(
        lines,
        &format!("Attached: {}", if session.attached { "yes" } else { "no" }),
    );
    push(lines, "");
    push(lines, "Captured output");

    if session.last_output.trim().is_empty() {
        push(lines, "(no output captured)");
    } else {
        for line in session.last_output.lines() {
            let t = truncate_chars(line, inner.saturating_sub(2));
            lines.push(cockpit_boxline(&t, t.len(), inner));
        }
    }

    if let Some(decision) = &ui_state.last_decision
        && decision.vm_name == vm.name
        && decision.session_name == session.session_name
    {
        push(lines, "");
        push(lines, "Prepared proposal");
        for line in decision.summary().lines() {
            push(lines, line);
        }
    }
}

fn cockpit_render_projects_view(lines: &mut Vec<String>, inner: usize) -> Result<()> {
    let dashboard = FleetDashboardSummary::load(Some(default_dashboard_path()))?;
    let text = render_project_list(&dashboard);
    for line in text.lines() {
        lines.push(cockpit_boxline(line, line.len(), inner));
    }
    Ok(())
}

fn cockpit_render_editor_view(ui_state: &FleetTuiUiState, lines: &mut Vec<String>, inner: usize) {
    let push = |lines: &mut Vec<String>, text: &str| {
        lines.push(cockpit_boxline(text, text.len(), inner));
    };

    push(lines, "Action Editor");
    push(lines, "");

    let Some(decision) = &ui_state.editor_decision else {
        push(lines, "No proposal loaded into the editor.");
        push(
            lines,
            "Press 'e' after preparing a proposal for the selected session.",
        );
        return;
    };

    push(
        lines,
        &format!("Target: {}/{}", decision.vm_name, decision.session_name),
    );
    push(lines, &format!("Action: {}", decision.action.as_str()));
    push(
        lines,
        &format!("Confidence: {:.0}%", decision.confidence * 100.0),
    );
    push(lines, &format!("Reasoning: {}", decision.reasoning));
    push(lines, "");
    push(lines, "Edited input");
    if decision.input_text.is_empty() {
        push(lines, "(empty)");
    } else {
        for line in decision.input_text.lines() {
            let t = truncate_chars(line, inner.saturating_sub(2));
            lines.push(cockpit_boxline(&t, t.len(), inner));
        }
    }
    push(lines, "");
    push(
        lines,
        "e reload  i edit input  t cycle action  A apply edited",
    );
}

fn cockpit_render_new_session_view(
    state: &FleetState,
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) {
    let push = |lines: &mut Vec<String>, text: &str| {
        lines.push(cockpit_boxline(text, text.len(), inner));
    };

    push(lines, "New Session");
    push(lines, "");
    push(
        lines,
        &format!("Agent type: {}", ui_state.new_session_agent.as_str()),
    );
    push(lines, "");
    push(lines, "Running VMs");

    let running_vms = FleetTuiUiState::new_session_vm_refs(state);
    if running_vms.is_empty() {
        push(lines, "No running VMs available.");
    } else {
        for vm_name in &running_vms {
            let marker = if ui_state.new_session_vm.as_deref() == Some(vm_name.as_str()) {
                ">"
            } else {
                " "
            };
            let row = format!("  {marker} {vm_name}");
            lines.push(cockpit_boxline(&row, row.len(), inner));
        }
    }
    push(lines, "");
    push(
        lines,
        "n jump here | j/k choose VM | t cycle agent | Enter create",
    );
}

fn collect_observed_fleet_state(azlin_path: &Path, capture_lines: usize) -> Result<FleetState> {
    let mut state = FleetState::new(azlin_path.to_path_buf());
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh();

    for vm in state
        .vms
        .iter_mut()
        .filter(|vm| vm.is_running() && existing_vms.iter().any(|name| name == &vm.name))
    {
        vm.tmux_sessions = FleetState::poll_tmux_sessions_with_path(azlin_path, &vm.name);
    }

    let mut observer = FleetObserver::new(azlin_path.to_path_buf());
    observer.capture_lines = capture_lines.clamp(1, MAX_CAPTURE_LINES);
    let adopter = SessionAdopter::new(azlin_path.to_path_buf());
    for vm in state.vms.iter_mut().filter(|vm| vm.is_running()) {
        let discovered = adopter.discover_sessions(&vm.name);
        for session in &mut vm.tmux_sessions {
            if let Some(metadata) = discovered
                .iter()
                .find(|candidate| candidate.session_name == session.session_name)
            {
                session.working_directory = metadata.working_directory.clone();
                session.repo_url = metadata.inferred_repo.clone();
                session.git_branch = metadata.inferred_branch.clone();
                session.pr_url = metadata.inferred_pr.clone();
                session.task_summary = metadata.inferred_task.clone();
            }
        }
        for session in &mut vm.tmux_sessions {
            let observation = observer.observe_session(&vm.name, &session.session_name)?;
            session.agent_status = observation.status;
            session.last_output = observation.last_output_lines.join("\n");
        }
    }

    Ok(state)
}

fn run_tui_dry_run(
    azlin_path: &Path,
    state: &FleetState,
    ui_state: &mut FleetTuiUiState,
) -> Result<()> {
    let Some((vm, session)) = ui_state.selected_session(state) else {
        ui_state.status_message = Some("No session selected for dry-run.".to_string());
        return Ok(());
    };

    let backend = NativeReasonerBackend::detect("auto")?;
    let mut reasoner = FleetSessionReasoner::new(azlin_path.to_path_buf(), backend);
    let analysis = reasoner.reason_about_session(
        &vm.name,
        &session.session_name,
        "",
        "",
        Some(&session.last_output),
    )?;
    let summary = format!(
        "Prepared proposal for {}/{}: {} ({:.0}%)",
        analysis.decision.vm_name,
        analysis.decision.session_name,
        analysis.decision.action.as_str(),
        analysis.decision.confidence * 100.0
    );
    ui_state.last_decision = Some(analysis.decision);
    ui_state.status_message = Some(summary);
    ui_state.tab = FleetTuiTab::Detail;
    Ok(())
}

fn run_tui_apply(azlin_path: &Path, ui_state: &mut FleetTuiUiState) -> Result<()> {
    let Some(decision) = ui_state.last_decision.clone() else {
        ui_state.status_message = Some("No prepared proposal to apply.".to_string());
        return Ok(());
    };

    let reasoner = FleetSessionReasoner::new(azlin_path.to_path_buf(), NativeReasonerBackend::None);
    match reasoner.execute_decision(&decision) {
        Ok(()) => {
            ui_state.status_message = Some(format!(
                "Applied {} to {}/{}.",
                decision.action.as_str(),
                decision.vm_name,
                decision.session_name
            ));
            Ok(())
        }
        Err(error) => {
            ui_state.status_message = Some(format!("Apply failed: {error}"));
            Ok(())
        }
    }
}

fn run_tui_edit(ui_state: &mut FleetTuiUiState) {
    ui_state.load_selected_proposal_into_editor();
}

fn run_tui_edit_input(
    ui_state: &mut FleetTuiUiState,
    terminal_guard: &DashboardTerminalGuard,
) -> Result<()> {
    let Some(current_text) = ui_state
        .editor_decision
        .as_ref()
        .map(|decision| decision.input_text.clone())
    else {
        ui_state.status_message = Some("No editor proposal loaded. Press 'e' first.".to_string());
        return Ok(());
    };

    let edited = terminal_guard.prompt_line(&format!(
        "Edit input for the prepared proposal. Use \\n for newlines.\nCurrent: {}\nNew input: ",
        truncate_chars(&current_text.replace('\n', "\\n"), 120)
    ))?;
    if let Some(decision) = ui_state.editor_decision.as_mut() {
        decision.input_text = edited.replace("\\n", "\n");
        ui_state.status_message = Some(format!(
            "Updated editor input for {}/{}.",
            decision.vm_name, decision.session_name
        ));
    }
    Ok(())
}

fn run_tui_apply_edited(azlin_path: &Path, ui_state: &mut FleetTuiUiState) -> Result<()> {
    let Some(decision) = ui_state.editor_decision.clone() else {
        ui_state.status_message =
            Some("No edited proposal to apply. Press 'e' to open the editor.".to_string());
        return Ok(());
    };

    let reasoner = FleetSessionReasoner::new(azlin_path.to_path_buf(), NativeReasonerBackend::None);
    match reasoner.execute_decision(&decision) {
        Ok(()) => {
            ui_state.last_decision = Some(decision.clone());
            ui_state.tab = FleetTuiTab::Detail;
            ui_state.status_message = Some(format!(
                "Applied edited {} to {}/{}.",
                decision.action.as_str(),
                decision.vm_name,
                decision.session_name
            ));
            Ok(())
        }
        Err(error) => {
            ui_state.status_message = Some(format!("Edited apply failed: {error}"));
            Ok(())
        }
    }
}

fn run_tui_adopt_selected_session(
    azlin_path: &Path,
    state: &FleetState,
    ui_state: &mut FleetTuiUiState,
) -> Result<()> {
    let Some((vm, session)) = ui_state.selected_session(state) else {
        ui_state.status_message = Some("No session selected to adopt.".to_string());
        return Ok(());
    };

    let mut queue = TaskQueue::load(Some(default_queue_path()))?;
    if queue.has_active_assignment(&vm.name, &session.session_name) {
        ui_state.status_message = Some(format!(
            "{}/{} is already adopted into the active fleet queue.",
            vm.name, session.session_name
        ));
        return Ok(());
    }

    let adopter = SessionAdopter::new(azlin_path.to_path_buf());
    let adopted = adopter.adopt_sessions(
        &vm.name,
        &mut queue,
        Some(std::slice::from_ref(&session.session_name)),
    )?;
    if adopted.is_empty() {
        ui_state.status_message = Some(format!(
            "No adoptable session found for {}/{}.",
            vm.name, session.session_name
        ));
        return Ok(());
    }

    ui_state.status_message = Some(format!(
        "Adopted {}/{} into the fleet queue.",
        vm.name, session.session_name
    ));
    Ok(())
}

fn run_tui_create_session(azlin_path: &Path, ui_state: &mut FleetTuiUiState) -> Result<()> {
    let Some(vm_name) = ui_state.new_session_vm.as_deref() else {
        ui_state.status_message = Some("No running VM selected for session creation.".to_string());
        return Ok(());
    };
    validate_vm_name(vm_name)?;

    let session_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 10_000;
    let agent = ui_state.new_session_agent.as_str();
    let session_name = format!("{agent}-{session_suffix:04}");
    validate_session_name(&session_name)?;

    let remote_cmd = format!(
        "tmux new-session -d -s {} {}",
        shell_single_quote(&session_name),
        shell_single_quote(&format!("amplihack {agent}"))
    );
    let mut cmd = Command::new(azlin_path);
    cmd.args(["connect", vm_name, "--no-tmux", "--", &remote_cmd]);
    let output = run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT)?;
    if output.status.success() {
        ui_state.status_message = Some(format!(
            "Created session '{session_name}' on {vm_name} running {agent}."
        ));
        ui_state.tab = FleetTuiTab::Fleet;
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        ui_state.status_message = Some(if detail.is_empty() {
            format!("Failed to create {agent} session on {vm_name}.")
        } else {
            format!("Failed to create {agent} session on {vm_name}: {detail}")
        });
    }
    Ok(())
}

enum FleetTuiRow<'a> {
    Session(&'a VmInfo, &'a TmuxSessionInfo),
    Placeholder(&'a VmInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FleetSubview {
    #[default]
    Managed,
    AllSessions,
}

impl FleetSubview {
    fn label(self) -> &'static str {
        match self {
            FleetSubview::Managed => "managed",
            FleetSubview::AllSessions => "all",
        }
    }

    fn title(self) -> &'static str {
        match self {
            FleetSubview::Managed => "Managed Sessions",
            FleetSubview::AllSessions => "All Sessions",
        }
    }

    fn next(self) -> Self {
        match self {
            FleetSubview::Managed => FleetSubview::AllSessions,
            FleetSubview::AllSessions => FleetSubview::Managed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FleetTuiTab {
    #[default]
    Fleet,
    Detail,
    Projects,
    Editor,
    NewSession,
}

impl FleetTuiTab {
    fn label(self) -> &'static str {
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
enum FleetNewSessionAgent {
    #[default]
    Claude,
    Copilot,
    Amplifier,
}

impl FleetNewSessionAgent {
    fn as_str(self) -> &'static str {
        match self {
            FleetNewSessionAgent::Claude => "claude",
            FleetNewSessionAgent::Copilot => "copilot",
            FleetNewSessionAgent::Amplifier => "amplifier",
        }
    }

    fn next(self) -> Self {
        match self {
            FleetNewSessionAgent::Claude => FleetNewSessionAgent::Copilot,
            FleetNewSessionAgent::Copilot => FleetNewSessionAgent::Amplifier,
            FleetNewSessionAgent::Amplifier => FleetNewSessionAgent::Claude,
        }
    }
}

/// Narrows the fleet view to sessions matching a particular status category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    /// Error or Stuck sessions only.
    Errors,
    /// Sessions awaiting user input.
    Waiting,
    /// Actively running or thinking sessions.
    Active,
}

impl StatusFilter {
    fn label(self) -> &'static str {
        match self {
            StatusFilter::Errors => "errors",
            StatusFilter::Waiting => "waiting",
            StatusFilter::Active => "active",
        }
    }

    fn matches(self, status: AgentStatus) -> bool {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FleetTuiSelection {
    vm_name: String,
    session_name: String,
}

#[derive(Debug, Clone, Default)]
struct FleetTuiUiState {
    tab: FleetTuiTab,
    fleet_subview: FleetSubview,
    selected: Option<FleetTuiSelection>,
    last_decision: Option<SessionDecision>,
    editor_decision: Option<SessionDecision>,
    new_session_vm: Option<String>,
    new_session_agent: FleetNewSessionAgent,
    status_message: Option<String>,
    /// When true, the `?` help overlay is shown instead of the normal content.
    show_help: bool,
    /// Optional filter applied to the fleet view (shows only matching sessions).
    status_filter: Option<StatusFilter>,
}

impl FleetTuiUiState {
    fn sync_to_state(&mut self, state: &FleetState) {
        let sessions = self.session_refs(state);
        if sessions.is_empty() {
            self.selected = None;
        } else {
            let selected_exists = self
                .selected
                .as_ref()
                .is_some_and(|selected| sessions.iter().any(|candidate| candidate == selected));
            if !selected_exists {
                self.selected = sessions.into_iter().next();
            }
        }

        let running_vms = Self::new_session_vm_refs(state);
        if running_vms.is_empty() {
            self.new_session_vm = None;
        } else {
            let selected_exists = self
                .new_session_vm
                .as_ref()
                .is_some_and(|selected| running_vms.iter().any(|candidate| candidate == selected));
            if !selected_exists {
                self.new_session_vm = running_vms.into_iter().next();
            }
        }
    }

    fn move_selection(&mut self, state: &FleetState, delta: isize) {
        if self.tab == FleetTuiTab::NewSession {
            self.move_new_session_target(state, delta);
            return;
        }

        let sessions = self.session_refs(state);
        if sessions.is_empty() {
            self.selected = None;
            return;
        }

        let current_index = self
            .selected
            .as_ref()
            .and_then(|selected| sessions.iter().position(|candidate| candidate == selected))
            .unwrap_or(0);
        let len = sessions.len() as isize;
        let next = (current_index as isize + delta).rem_euclid(len) as usize;
        self.selected = sessions.get(next).cloned();
    }

    fn move_new_session_target(&mut self, state: &FleetState, delta: isize) {
        let running_vms = Self::new_session_vm_refs(state);
        if running_vms.is_empty() {
            self.new_session_vm = None;
            return;
        }

        let current_index = self
            .new_session_vm
            .as_ref()
            .and_then(|selected| {
                running_vms
                    .iter()
                    .position(|candidate| candidate == selected)
            })
            .unwrap_or(0);
        let len = running_vms.len() as isize;
        let next = (current_index as isize + delta).rem_euclid(len) as usize;
        self.new_session_vm = running_vms.get(next).cloned();
    }

    fn selection_matches(&self, vm_name: &str, session_name: &str) -> bool {
        self.selected.as_ref().is_some_and(|selected| {
            selected.vm_name == vm_name && selected.session_name == session_name
        })
    }

    fn selected_session<'a>(
        &self,
        state: &'a FleetState,
    ) -> Option<(&'a VmInfo, &'a TmuxSessionInfo)> {
        let selected = self.selected.as_ref()?;
        for vm in Self::fleet_vms(state, self.fleet_subview)
            .into_iter()
            .filter(|vm| vm.is_running())
        {
            if vm.name != selected.vm_name {
                continue;
            }
            if let Some(session) = vm
                .tmux_sessions
                .iter()
                .find(|session| session.session_name == selected.session_name)
            {
                return Some((vm, session));
            }
        }
        None
    }

    fn session_refs(&self, state: &FleetState) -> Vec<FleetTuiSelection> {
        let mut sessions = Vec::new();
        for vm in Self::fleet_vms(state, self.fleet_subview)
            .into_iter()
            .filter(|vm| vm.is_running())
        {
            for session in &vm.tmux_sessions {
                if self
                    .status_filter
                    .is_some_and(|filter| !filter.matches(session.agent_status))
                {
                    continue;
                }
                sessions.push(FleetTuiSelection {
                    vm_name: vm.name.clone(),
                    session_name: session.session_name.clone(),
                });
            }
        }
        sessions
    }

    fn fleet_vms(state: &FleetState, subview: FleetSubview) -> Vec<&VmInfo> {
        match subview {
            FleetSubview::Managed => state.managed_vms(),
            FleetSubview::AllSessions => state.all_vms(),
        }
    }

    fn new_session_vm_refs(state: &FleetState) -> Vec<String> {
        state
            .managed_vms()
            .into_iter()
            .filter(|vm| vm.is_running())
            .map(|vm| vm.name.clone())
            .collect()
    }

    /// Advance to the next tab, wrapping around.
    fn cycle_tab_forward(&mut self) {
        self.tab = match self.tab {
            FleetTuiTab::Fleet => FleetTuiTab::Detail,
            FleetTuiTab::Detail => FleetTuiTab::Projects,
            FleetTuiTab::Projects => FleetTuiTab::Editor,
            FleetTuiTab::Editor => FleetTuiTab::NewSession,
            FleetTuiTab::NewSession => FleetTuiTab::Fleet,
        };
    }

    /// Retreat to the previous tab, wrapping around.
    fn cycle_tab_backward(&mut self) {
        self.tab = match self.tab {
            FleetTuiTab::Fleet => FleetTuiTab::NewSession,
            FleetTuiTab::Detail => FleetTuiTab::Fleet,
            FleetTuiTab::Projects => FleetTuiTab::Detail,
            FleetTuiTab::Editor => FleetTuiTab::Projects,
            FleetTuiTab::NewSession => FleetTuiTab::Editor,
        };
    }

    /// Set or clear a status filter.  Calling with the same filter clears it (toggle).
    fn toggle_filter(&mut self, filter: StatusFilter) {
        if self.status_filter == Some(filter) {
            self.status_filter = None;
        } else {
            self.status_filter = Some(filter);
        }
    }

    fn load_selected_proposal_into_editor(&mut self) {
        let Some(selected) = self.selected.as_ref() else {
            self.status_message = Some("No session selected for editing.".to_string());
            return;
        };
        let Some(decision) = self.last_decision.as_ref().filter(|decision| {
            decision.vm_name == selected.vm_name && decision.session_name == selected.session_name
        }) else {
            self.status_message =
                Some("No prepared proposal for the selected session.".to_string());
            return;
        };
        self.editor_decision = Some(decision.clone());
        self.tab = FleetTuiTab::Editor;
        self.status_message = Some(format!(
            "Loaded proposal into editor for {}/{}.",
            decision.vm_name, decision.session_name
        ));
    }

    fn cycle_editor_action(&mut self) {
        let Some(decision) = self.editor_decision.as_mut() else {
            self.status_message = Some("No editor proposal loaded. Press 'e' first.".to_string());
            return;
        };
        decision.action = decision.action.next();
        self.status_message = Some(format!(
            "Editor action set to {} for {}/{}.",
            decision.action.as_str(),
            decision.vm_name,
            decision.session_name
        ));
    }

    fn cycle_new_session_agent(&mut self) {
        self.new_session_agent = self.new_session_agent.next();
        self.status_message = Some(format!(
            "New session agent set to {}.",
            self.new_session_agent.as_str()
        ));
    }

    fn cycle_fleet_subview(&mut self, state: &FleetState) {
        self.fleet_subview = self.fleet_subview.next();
        self.sync_to_state(state);
        self.status_message = Some(format!("Fleet view set to {}.", self.fleet_subview.title()));
    }

    fn navigate_back(&mut self) {
        self.tab = match self.tab {
            FleetTuiTab::Editor => FleetTuiTab::Detail,
            FleetTuiTab::NewSession => FleetTuiTab::Fleet,
            FleetTuiTab::Detail | FleetTuiTab::Projects => FleetTuiTab::Fleet,
            FleetTuiTab::Fleet => FleetTuiTab::Fleet,
        };
    }

    fn skip_selected_proposal(&mut self) {
        let Some(selected) = self.selected.as_ref() else {
            self.status_message = Some("No session selected to skip.".to_string());
            return;
        };

        let matches_selected = |decision: &SessionDecision| {
            decision.vm_name == selected.vm_name && decision.session_name == selected.session_name
        };

        let had_prepared = self.last_decision.as_ref().is_some_and(matches_selected)
            || self.editor_decision.as_ref().is_some_and(matches_selected);
        if !had_prepared {
            self.status_message = Some("No prepared proposal to skip.".to_string());
            return;
        }

        if self.last_decision.as_ref().is_some_and(matches_selected) {
            self.last_decision = None;
        }
        if self.editor_decision.as_ref().is_some_and(matches_selected) {
            self.editor_decision = None;
        }
        self.tab = FleetTuiTab::Detail;
        self.status_message = Some("Skipped.".to_string());
    }
}

/// Compute aggregate session status counts for the fleet header.
///
/// Returns `(total, active, waiting, errors, idle)`.
fn fleet_status_summary(
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
fn status_sort_priority(status: AgentStatus) -> u8 {
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
struct DashboardTerminalGuard {
    fd: i32,
    original: Option<libc::termios>,
}

#[cfg(unix)]
impl DashboardTerminalGuard {
    fn activate() -> Result<Self> {
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

    fn prompt_line(&self, prompt: &str) -> Result<String> {
        if let Some(original) = self.original
            && unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &original) } != 0
        {
            bail!("failed to restore dashboard terminal mode for prompt");
        }

        let mut stdout = io::stdout();
        write!(stdout, "{SHOW_CURSOR}\r\n{prompt}")?;
        stdout.flush().context("failed to flush dashboard prompt")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read dashboard prompt input")?;

        if let Some(original) = self.original {
            let mut raw = original;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &raw) } != 0 {
                bail!("failed to restore dashboard raw mode after prompt");
            }
        }

        write!(stdout, "{HIDE_CURSOR}")?;
        stdout
            .flush()
            .context("failed to flush dashboard prompt cleanup")?;

        Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
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
struct DashboardTerminalGuard;

#[cfg(not(unix))]
impl DashboardTerminalGuard {
    fn activate() -> Result<Self> {
        Ok(Self)
    }

    fn prompt_line(&self, prompt: &str) -> Result<String> {
        let mut stdout = io::stdout();
        write!(stdout, "\r\n{prompt}")?;
        stdout.flush().context("failed to flush dashboard prompt")?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read dashboard prompt input")?;
        Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
    }
}

#[cfg(not(unix))]
impl Drop for DashboardTerminalGuard {
    fn drop(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardKey {
    Char(char),
    Left,
    Right,
    Up,
    Down,
}

fn decode_dashboard_key_bytes(bytes: &[u8]) -> Option<DashboardKey> {
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
fn read_dashboard_key(timeout: Duration) -> Option<DashboardKey> {
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
fn read_dashboard_key(timeout: Duration) -> Option<DashboardKey> {
    thread::sleep(timeout);
    None
}

fn default_queue_path() -> PathBuf {
    fleet_home_dir().join("task_queue.json")
}

fn default_dashboard_path() -> PathBuf {
    fleet_home_dir().join("dashboard.json")
}

fn default_log_dir() -> PathBuf {
    fleet_home_dir().join("logs")
}

fn default_coordination_dir() -> PathBuf {
    fleet_home_dir().join("coordination")
}

fn default_projects_path() -> PathBuf {
    fleet_home_dir().join("projects.toml")
}

fn default_graph_path() -> PathBuf {
    fleet_home_dir().join("graph.json")
}

fn default_copilot_lock_dir() -> PathBuf {
    claude_project_dir()
        .join(".claude")
        .join("runtime")
        .join("locks")
}

fn default_copilot_log_dir() -> PathBuf {
    claude_project_dir()
        .join(".claude")
        .join("runtime")
        .join("copilot-decisions")
}

fn fleet_home_dir() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".amplihack").join("fleet")
}

fn claude_project_dir() -> PathBuf {
    env::var_os("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn shell_single_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn first_matching_pattern(patterns: &[&str], text: &str, multiline: bool) -> Option<String> {
    patterns.iter().find_map(|pattern| {
        RegexBuilder::new(pattern)
            .case_insensitive(true)
            .multi_line(multiline)
            .build()
            .ok()
            .filter(|regex: &Regex| regex.is_match(text))
            .map(|_| (*pattern).to_string())
    })
}

fn auth_files_for_service(
    service: &str,
) -> Option<&'static [(&'static str, &'static str, &'static str)]> {
    match service {
        "github" => Some(AUTH_GITHUB_FILES),
        "azure" => Some(AUTH_AZURE_FILES),
        "claude" => Some(AUTH_CLAUDE_FILES),
        _ => None,
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest),
        None => PathBuf::from(path),
    }
}

fn parse_session_target(session_target: &str) -> (Option<String>, String) {
    if let Some((vm_name, session_name)) = session_target.split_once(':') {
        let vm_name = vm_name.trim();
        let session_name = session_name.trim().to_string();
        return (
            (!vm_name.is_empty()).then(|| vm_name.to_string()),
            session_name,
        );
    }
    (None, session_target.trim().to_string())
}

fn load_previous_scout(
    path: &Path,
) -> Result<(BTreeMap<String, String>, Vec<SessionDecisionRecord>)> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let statuses = value
        .get("session_statuses")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let decisions = value
        .get("decisions")
        .cloned()
        .map(serde_json::from_value::<Vec<SessionDecisionRecord>>)
        .transpose()
        .context("failed to decode previous scout decisions")?
        .unwrap_or_default();
    Ok((statuses, decisions))
}

fn write_json_file(path: &Path, payload: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = serde_json::to_vec_pretty(payload).context("failed to encode json payload")?;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
        .with_context(|| format!("failed to create temp file for {}", path.display()))?;
    temp.write_all(&rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    temp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("failed to persist {}", path.display()))?;
    Ok(())
}

fn render_scout_report(
    decisions: &[SessionDecisionRecord],
    all_vm_count: usize,
    running_vm_count: usize,
    adopted_count: usize,
    skip_adopt: bool,
) -> String {
    let mut action_counts = BTreeMap::<String, usize>::new();
    for decision in decisions {
        *action_counts.entry(decision.action.clone()).or_insert(0) += 1;
    }

    let mut lines = vec![
        "=".repeat(60),
        "FLEET SCOUT REPORT".to_string(),
        "=".repeat(60),
        format!("VMs discovered: {all_vm_count}"),
        format!("Running VMs: {running_vm_count}"),
        format!("Sessions analyzed: {}", decisions.len()),
        if skip_adopt {
            "Adoption: skipped".to_string()
        } else {
            format!("Adopted sessions: {adopted_count}")
        },
    ];

    if !action_counts.is_empty() {
        lines.push("Actions:".to_string());
        for (action, count) in action_counts {
            lines.push(format!("  {action}: {count}"));
        }
    }
    lines.push(String::new());

    for decision in decisions {
        let status_suffix = if decision.status.is_empty() {
            String::new()
        } else {
            format!(" [{}]", decision.status)
        };
        lines.push(format!(
            "  {}/{}{} -> {} ({:.0}%)",
            decision.vm,
            decision.session,
            status_suffix,
            decision.action,
            decision.confidence * 100.0
        ));
        if !decision.branch.is_empty() {
            lines.push(format!("    Branch: {}", decision.branch));
        }
        if !decision.pr.is_empty() {
            lines.push(format!("    PR: {}", decision.pr));
        }
        if !decision.project.is_empty() {
            lines.push(format!("    Project: {}", decision.project));
        }
        if let Some(error) = &decision.error {
            lines.push(format!("    ERROR: {error}"));
        } else {
            lines.push(format!("    Reason: {}", decision.reasoning));
            if !decision.input_text.is_empty() {
                lines.push(format!(
                    "    Input: {}",
                    truncate_chars(&decision.input_text.replace('\n', "\\n"), 120)
                ));
            }
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

fn render_advance_report(
    decisions: &[SessionDecisionRecord],
    executed: &[SessionExecutionRecord],
) -> String {
    let mut action_counts = BTreeMap::<String, usize>::new();
    for decision in decisions {
        *action_counts.entry(decision.action.clone()).or_insert(0) += 1;
    }

    let mut lines = vec![
        "=".repeat(60),
        "FLEET ADVANCE REPORT".to_string(),
        "=".repeat(60),
        format!("Sessions analyzed: {}", decisions.len()),
    ];
    for (action, count) in action_counts {
        lines.push(format!("  {action}: {count}"));
    }
    lines.push(String::new());

    for execution in executed {
        let label = if let Some(error) = &execution.error {
            format!("[ERROR] {error}")
        } else if execution.executed {
            "[OK]".to_string()
        } else {
            "[SKIPPED]".to_string()
        };
        lines.push(format!(
            "  {label} {}/{} -> {}",
            execution.vm, execution.session, execution.action
        ));
    }

    lines.join("\n")
}

fn find_reasoner_binary() -> Option<PathBuf> {
    if let Ok(path) = env::var("AMPLIHACK_FLEET_REASONER_BINARY_PATH") {
        let path = PathBuf::from(path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    if let Ok(info) = BinaryFinder::find("claude") {
        return Some(info.path);
    }

    if let Ok(path) = env::var("RUSTYCLAWD_PATH") {
        let path = PathBuf::from(path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    find_binary("claude-code")
}

fn match_project(repo_url: &str) -> Result<(String, Vec<ProjectObjective>)> {
    let projects = load_projects_registry(&default_projects_path())?;
    for (name, project) in projects {
        if !project.repo_url.is_empty()
            && project.repo_url.trim_end_matches('/') == repo_url.trim_end_matches('/')
        {
            return Ok((name, project.objectives));
        }
    }
    Ok((String::new(), Vec::new()))
}

fn gather_session_context(
    azlin_path: &Path,
    vm_name: &str,
    session_name: &str,
    task_prompt: &str,
    project_priorities: &str,
    cached_tmux_capture: Option<&str>,
) -> Result<SessionContext> {
    let mut context = SessionContext::new(vm_name, session_name, task_prompt, project_priorities)?;
    let quoted_session = shell_single_quote(session_name);
    let gather_cmd = format!(
        concat!(
            "echo \"===TMUX===\"; ",
            "tmux capture-pane -t {session} -p -S - 2>/dev/null || echo 'NO_SESSION'; ",
            "echo \"===CWD===\"; ",
            "CWD=$(tmux display-message -t {session} -p \"#{{pane_current_path}}\" 2>/dev/null); ",
            "echo \"$CWD\"; ",
            "echo \"===GIT===\"; ",
            "if [ -n \"$CWD\" ] && [ -d \"$CWD/.git\" ]; then ",
            "cd \"$CWD\"; ",
            "echo \"BRANCH:$(git branch --show-current 2>/dev/null)\"; ",
            "echo \"REMOTE:$(git remote get-url origin 2>/dev/null)\"; ",
            "echo \"MODIFIED:$(git diff --name-only HEAD 2>/dev/null | head -10 | tr '\\n' ',')\"; ",
            "PRURL=$(gh pr list --head \"$(git branch --show-current 2>/dev/null)\" --json url --jq \".[]|.url\" 2>/dev/null | head -1); ",
            "if [ -n \"$PRURL\" ]; then echo \"PR_URL:$PRURL\"; fi; ",
            "fi; ",
            "echo \"===TRANSCRIPT===\"; ",
            "if [ -n \"$CWD\" ]; then ",
            "PKEY=$(echo \"$CWD\" | sed \"s|/|-|g\"); ",
            "JSONL=$(ls -t \"$HOME/.claude/projects/$PKEY/\"*.jsonl 2>/dev/null | head -1); ",
            "if [ -n \"$JSONL\" ]; then ",
            "MSGS=$(grep -E '\"type\":\"(user|assistant)\"' \"$JSONL\" 2>/dev/null | grep -oP '\"text\":\"[^\"]*\"' | sed 's/\"text\":\"//;s/\"$//' | grep -v '^$'); ",
            "TOTAL=$(echo \"$MSGS\" | wc -l); ",
            "echo \"TRANSCRIPT_LINES:$TOTAL\"; ",
            "echo \"---EARLY---\"; ",
            "echo \"$MSGS\" | head -50; ",
            "echo \"---RECENT---\"; ",
            "echo \"$MSGS\" | tail -200; ",
            "fi; fi; ",
            "echo \"===HEALTH===\"; ",
            "MEM=$(free -m 2>/dev/null | grep Mem | awk '{{printf \"%.0f\", $3/$2*100}}'); ",
            "DISK=$(df -h / 2>/dev/null | tail -1 | awk '{{print $5}}' | tr -d \"%\"); ",
            "LOAD=$(cat /proc/loadavg 2>/dev/null | awk '{{print $1}}'); ",
            "echo \"mem=${{MEM:-?}}% disk=${{DISK:-?}}% load=${{LOAD:-?}}\"; ",
            "echo \"===OBJECTIVES===\"; ",
            "if [ -n \"$CWD\" ] && command -v gh >/dev/null 2>&1; then ",
            "REMOTE=$(cd \"$CWD\" 2>/dev/null && git remote get-url origin 2>/dev/null); ",
            "if [ -n \"$REMOTE\" ]; then ",
            "gh issue list --repo \"$REMOTE\" --label fleet-objective --json number,title,state --jq '.[]|[.number,.title,.state]|@tsv' 2>/dev/null; ",
            "fi; fi; ",
            "echo \"===END===\""
        ),
        session = quoted_session
    );

    let mut cmd = Command::new(azlin_path);
    cmd.args(["connect", vm_name, "--no-tmux", "--yes", "--", &gather_cmd]);

    match run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("===TMUX===")
                || stdout.contains("===END===")
                || output.status.success()
            {
                parse_context_output(&stdout, &mut context)?;
            }
        }
        Err(_) => {
            context.agent_status = AgentStatus::Unreachable;
        }
    }

    if let Some(cached_capture) = cached_tmux_capture {
        context.tmux_capture = cached_capture.to_string();
        context.agent_status = infer_agent_status(cached_capture);
    }

    Ok(context)
}

fn parse_context_output(output: &str, context: &mut SessionContext) -> Result<()> {
    let sections = output.split("===").collect::<Vec<_>>();
    let mut index = 0usize;
    while index + 1 < sections.len() {
        let label = sections[index].trim();
        if label.is_empty() {
            index += 1;
            continue;
        }
        let body = sections[index + 1].trim();
        match label {
            "TMUX" => {
                if body == "NO_SESSION" {
                    context.agent_status = AgentStatus::NoSession;
                } else {
                    context.tmux_capture = body.to_string();
                    context.agent_status = infer_agent_status(body);
                }
            }
            "CWD" => context.working_directory = body.to_string(),
            "GIT" => {
                for line in body.lines() {
                    if let Some(value) = line.strip_prefix("BRANCH:") {
                        context.git_branch = value.to_string();
                    } else if let Some(value) = line.strip_prefix("REMOTE:") {
                        context.repo_url = value.to_string();
                    } else if let Some(value) = line.strip_prefix("MODIFIED:") {
                        context.files_modified = value
                            .split(',')
                            .filter_map(|entry| {
                                let entry = entry.trim();
                                (!entry.is_empty()).then(|| entry.to_string())
                            })
                            .collect();
                    } else if let Some(value) = line.strip_prefix("PR_URL:") {
                        context.pr_url = value.trim().to_string();
                    }
                }
            }
            "TRANSCRIPT" => {
                let mut early = String::new();
                let mut recent = String::new();
                if let Some(early_start) = body.find("---EARLY---") {
                    if let Some(recent_start) = body.find("---RECENT---") {
                        early = body[early_start + "---EARLY---".len()..recent_start]
                            .trim()
                            .to_string();
                        recent = body[recent_start + "---RECENT---".len()..]
                            .trim()
                            .to_string();
                    }
                } else {
                    recent = body.to_string();
                }
                let mut transcript_parts = Vec::new();
                if !early.is_empty() {
                    transcript_parts.push("=== Session start ===".to_string());
                    transcript_parts.push(early);
                }
                if !recent.is_empty() {
                    if !transcript_parts.is_empty() {
                        transcript_parts.push("\n=== Recent activity ===".to_string());
                    }
                    transcript_parts.push(recent);
                }
                context.transcript_summary = transcript_parts.join("\n");
                if context.pr_url.is_empty() {
                    for line in context.transcript_summary.lines() {
                        if let Some(value) = line.split("PR_CREATED:").nth(1) {
                            context.pr_url = value.trim().to_string();
                            break;
                        }
                    }
                }
            }
            "HEALTH" => context.health_summary = body.to_string(),
            "OBJECTIVES" => {
                for line in body.lines() {
                    let parts = line.split('\t').collect::<Vec<_>>();
                    if parts.len() < 2 {
                        continue;
                    }
                    let number = match parts[0].trim().parse::<i64>() {
                        Ok(number) => number,
                        Err(_) => continue,
                    };
                    let title = parts[1]
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(256)
                        .collect::<String>();
                    let state = parts
                        .get(2)
                        .map(|value| value.trim().to_ascii_lowercase())
                        .filter(|value| value == "open" || value == "closed")
                        .unwrap_or_else(|| "open".to_string());
                    context.project_objectives.push(ProjectObjective {
                        number,
                        title,
                        state,
                        url: String::new(),
                    });
                }
            }
            _ => {}
        }
        index += 2;
    }

    if !context.repo_url.is_empty() {
        let (project_name, mut local_objectives) = match_project(&context.repo_url)?;
        if !project_name.is_empty() {
            context.project_name = project_name;
            let existing = context
                .project_objectives
                .iter()
                .map(|objective| objective.number)
                .collect::<std::collections::BTreeSet<_>>();
            local_objectives.retain(|objective| !existing.contains(&objective.number));
            context.project_objectives.extend(local_objectives);
        }
    }

    Ok(())
}

fn infer_agent_status(tmux_text: &str) -> AgentStatus {
    let lines = tmux_text.trim().lines().collect::<Vec<_>>();
    let combined = lines.join("\n");
    let combined_lower = combined.to_ascii_lowercase();
    let last_line = lines
        .last()
        .map(|line| line.trim())
        .unwrap_or_default()
        .to_string();
    let last_line_lower = last_line.to_ascii_lowercase();

    let mut prompt_line_text = String::new();
    let mut has_prompt = false;
    for line in lines.iter().rev() {
        let stripped = line.trim();
        if stripped.starts_with('\u{276f}') {
            has_prompt = true;
            prompt_line_text = stripped.trim_start_matches('\u{276f}').trim().to_string();
            break;
        }
    }

    if lines
        .iter()
        .any(|line| line.contains("(running)") && line.contains("\u{23f5}\u{23f5}"))
    {
        return AgentStatus::Running;
    }
    if lines
        .iter()
        .any(|line| line.trim_start().starts_with('\u{00b7}'))
    {
        return AgentStatus::Thinking;
    }

    for line in lines.iter().rev() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with('\u{25cf}') && !stripped.starts_with("\u{25cf} Bash(") {
            return AgentStatus::Thinking;
        }
        if stripped.starts_with('\u{23bf}') {
            return AgentStatus::Thinking;
        }
        break;
    }

    let has_finished_indicator = lines.iter().any(|line| line.contains('\u{273b}'));
    if has_finished_indicator && has_prompt {
        return if prompt_line_text.is_empty() {
            AgentStatus::Idle
        } else {
            AgentStatus::Thinking
        };
    }
    if has_finished_indicator {
        return AgentStatus::Thinking;
    }

    if ["thinking...", "running:", "loading"]
        .iter()
        .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::Thinking;
    }
    if combined.contains("\u{25cf} Bash(")
        || combined.contains("\u{25cf} Read(")
        || combined.contains("\u{25cf} Write(")
        || combined.contains("\u{25cf} Edit(")
    {
        if last_line.contains("\u{23f5}\u{23f5}") {
            return AgentStatus::WaitingInput;
        }
        return AgentStatus::Thinking;
    }
    if has_prompt && !prompt_line_text.is_empty() {
        return AgentStatus::Thinking;
    }
    if has_prompt {
        return AgentStatus::Idle;
    }
    if last_line_lower.ends_with("$") || last_line_lower.ends_with("$ ") {
        return AgentStatus::Shell;
    }
    if ["y/n]", "yes/no", "[y/n", "(yes/no)"]
        .iter()
        .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::WaitingInput;
    }
    if combined.contains("\u{23f5}\u{23f5}")
        && (combined_lower.contains("bypass") || combined_lower.contains("allow"))
    {
        return AgentStatus::WaitingInput;
    }
    if last_line_lower.ends_with('?') {
        return AgentStatus::WaitingInput;
    }
    if ["error:", "traceback", "fatal:", "panic:"]
        .iter()
        .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::Error;
    }
    if combined.contains("GOAL_STATUS: ACHIEVED") || combined.contains("Workflow Complete") {
        return AgentStatus::Completed;
    }
    if (combined.contains("gh pr create")
        || combined.contains("PR #")
        || combined_lower.contains("pull request"))
        && ["created", "opened", "merged"]
            .iter()
            .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::Completed;
    }
    if combined.trim().len() > MIN_SUBSTANTIAL_OUTPUT_LEN {
        return AgentStatus::Running;
    }
    AgentStatus::Unknown
}

fn parse_reasoner_response(
    response_text: &str,
    context: &SessionContext,
) -> Option<SessionDecision> {
    let json_start = response_text.find('{')?;
    let json_end = response_text.rfind('}')?;
    if json_end <= json_start {
        return None;
    }
    let value: Value = serde_json::from_str(&response_text[json_start..=json_end]).ok()?;
    let action = match value.get("action").and_then(Value::as_str) {
        Some("send_input") => SessionAction::SendInput,
        Some("wait") => SessionAction::Wait,
        Some("escalate") => SessionAction::Escalate,
        Some("mark_complete") => SessionAction::MarkComplete,
        Some("restart") => SessionAction::Restart,
        _ => SessionAction::Wait,
    };
    let confidence = value
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.5)
        .clamp(0.0, 1.0);
    Some(SessionDecision {
        session_name: context.session_name.clone(),
        vm_name: context.vm_name.clone(),
        action,
        input_text: value
            .get("input_text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        reasoning: value
            .get("reasoning")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        confidence,
    })
}

fn heuristic_decision(context: &SessionContext) -> SessionDecision {
    let (action, reasoning, confidence) = match context.agent_status {
        AgentStatus::Completed => (
            SessionAction::MarkComplete,
            "Session output indicates completion".to_string(),
            CONFIDENCE_COMPLETION,
        ),
        AgentStatus::Error | AgentStatus::Shell | AgentStatus::Stuck => (
            SessionAction::Escalate,
            "Session needs human attention or restart review".to_string(),
            CONFIDENCE_ERROR,
        ),
        AgentStatus::WaitingInput => (
            SessionAction::Wait,
            "Session is waiting for input, but no native reasoner backend was available"
                .to_string(),
            CONFIDENCE_IDLE,
        ),
        AgentStatus::Thinking | AgentStatus::Running => (
            SessionAction::Wait,
            "Session appears active; no intervention needed".to_string(),
            CONFIDENCE_RUNNING,
        ),
        AgentStatus::Idle => (
            SessionAction::Wait,
            "Session is idle at the prompt".to_string(),
            CONFIDENCE_IDLE,
        ),
        AgentStatus::NoSession | AgentStatus::Unreachable | AgentStatus::Unknown => (
            SessionAction::Wait,
            "Session is empty or unavailable".to_string(),
            CONFIDENCE_UNKNOWN,
        ),
    };
    SessionDecision {
        session_name: context.session_name.clone(),
        vm_name: context.vm_name.clone(),
        action,
        input_text: String::new(),
        reasoning,
        confidence,
    }
}

fn is_dangerous_input(text: &str) -> bool {
    if Regex::new(r"[;|&`]|\$\(")
        .ok()
        .is_some_and(|regex| regex.is_match(text))
    {
        return true;
    }
    if SAFE_INPUT_PATTERNS
        .iter()
        .filter_map(|pattern| {
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()
        })
        .any(|regex| regex.is_match(text))
    {
        return false;
    }
    DANGEROUS_INPUT_PATTERNS
        .iter()
        .filter_map(|pattern| {
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()
        })
        .any(|regex| regex.is_match(text))
}

fn remote_parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_else(|| ".".to_string())
}

fn validate_chmod_mode(mode: &str) -> Result<()> {
    if mode.len() < 3 || mode.len() > 4 || !mode.chars().all(|ch| ('0'..='7').contains(&ch)) {
        bail!("Invalid chmod mode: {mode:?}");
    }
    Ok(())
}

fn validate_vm_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Invalid VM name: {name:?}");
    };
    if !first.is_ascii_alphanumeric() {
        bail!("Invalid VM name: {name:?}");
    }
    if name.len() > 64 || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
        bail!("Invalid VM name: {name:?}");
    }
    Ok(())
}

fn validate_session_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Invalid session name: {name:?}");
    };
    if !first.is_ascii_alphanumeric() {
        bail!("Invalid session name: {name:?}");
    }
    if name.len() > 128
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-'))
    {
        bail!("Invalid session name: {name:?}");
    }
    Ok(())
}

fn get_azlin_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("AZLIN_PATH") {
        return Ok(PathBuf::from(path));
    }

    if let Some(path) = find_binary("azlin") {
        return Ok(path);
    }

    if let Some(home) = env::var_os("HOME") {
        let dev_path = PathBuf::from(home).join("src/azlin/.venv/bin/azlin");
        if is_executable_file(&dev_path) {
            return Ok(dev_path);
        }
    }

    bail!(
        "azlin not found. Set AZLIN_PATH to the binary location.\nSee: https://github.com/rysweet/azlin"
    )
}

fn find_binary(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        let candidate = dir.join(name);
        is_executable_file(&candidate).then_some(candidate)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskPriority {
    Critical,
    High,
    Medium,
    Low,
}

impl TaskPriority {
    fn as_name(self) -> &'static str {
        match self {
            TaskPriority::Critical => "CRITICAL",
            TaskPriority::High => "HIGH",
            TaskPriority::Medium => "MEDIUM",
            TaskPriority::Low => "LOW",
        }
    }

    fn short_label(self) -> char {
        match self {
            TaskPriority::Critical => 'C',
            TaskPriority::High => 'H',
            TaskPriority::Medium => 'M',
            TaskPriority::Low => 'L',
        }
    }

    fn from_name(value: &str) -> Self {
        match value {
            "CRITICAL" => TaskPriority::Critical,
            "HIGH" => TaskPriority::High,
            "LOW" => TaskPriority::Low,
            _ => TaskPriority::Medium,
        }
    }

    fn rank(self) -> u8 {
        match self {
            TaskPriority::Critical => 0,
            TaskPriority::High => 1,
            TaskPriority::Medium => 2,
            TaskPriority::Low => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStatus {
    Queued,
    Assigned,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Assigned => "assigned",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            TaskStatus::Queued => "QUEUED",
            TaskStatus::Assigned => "ASSIGNED",
            TaskStatus::Running => "RUNNING",
            TaskStatus::Completed => "COMPLETED",
            TaskStatus::Failed => "FAILED",
            TaskStatus::Cancelled => "CANCELLED",
        }
    }

    fn from_value(value: &str) -> Self {
        match value {
            "assigned" => TaskStatus::Assigned,
            "running" => TaskStatus::Running,
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            "cancelled" => TaskStatus::Cancelled,
            _ => TaskStatus::Queued,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FleetTask {
    id: String,
    prompt: String,
    repo_url: String,
    branch: String,
    priority: TaskPriority,
    status: TaskStatus,
    agent_command: String,
    agent_mode: String,
    max_turns: u32,
    protected: bool,
    assigned_vm: Option<String>,
    assigned_session: Option<String>,
    assigned_at: Option<String>,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    result: Option<String>,
    pr_url: Option<String>,
    error: Option<String>,
}

impl FleetTask {
    fn new(
        prompt: &str,
        repo_url: &str,
        priority: TaskPriority,
        agent_command: &str,
        agent_mode: &str,
        max_turns: u32,
    ) -> Self {
        Self {
            id: generate_task_id(prompt),
            prompt: prompt.to_string(),
            repo_url: repo_url.to_string(),
            branch: String::new(),
            priority,
            status: TaskStatus::Queued,
            agent_command: agent_command.to_string(),
            agent_mode: agent_mode.to_string(),
            max_turns,
            protected: false,
            assigned_vm: None,
            assigned_session: None,
            assigned_at: None,
            created_at: now_isoformat(),
            started_at: None,
            completed_at: None,
            result: None,
            pr_url: None,
            error: None,
        }
    }

    fn to_json_value(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "prompt": self.prompt,
            "repo_url": self.repo_url,
            "branch": self.branch,
            "priority": self.priority.as_name(),
            "status": self.status.as_str(),
            "agent_command": self.agent_command,
            "agent_mode": self.agent_mode,
            "max_turns": self.max_turns,
            "protected": self.protected,
            "assigned_vm": self.assigned_vm,
            "assigned_session": self.assigned_session,
            "assigned_at": self.assigned_at,
            "created_at": self.created_at,
            "started_at": self.started_at,
            "completed_at": self.completed_at,
            "result": self.result,
            "pr_url": self.pr_url,
            "error": self.error,
        })
    }

    fn from_json_value(value: &Value) -> Option<Self> {
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            prompt: value.get("prompt")?.as_str()?.to_string(),
            repo_url: value
                .get("repo_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            branch: value
                .get("branch")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            priority: TaskPriority::from_name(
                value
                    .get("priority")
                    .and_then(Value::as_str)
                    .unwrap_or("MEDIUM"),
            ),
            status: TaskStatus::from_value(
                value
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("queued"),
            ),
            agent_command: value
                .get("agent_command")
                .and_then(Value::as_str)
                .unwrap_or("claude")
                .to_string(),
            agent_mode: value
                .get("agent_mode")
                .and_then(Value::as_str)
                .unwrap_or("auto")
                .to_string(),
            max_turns: value
                .get("max_turns")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_MAX_TURNS as u64) as u32,
            protected: value
                .get("protected")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            assigned_vm: value
                .get("assigned_vm")
                .and_then(Value::as_str)
                .map(str::to_string),
            assigned_session: value
                .get("assigned_session")
                .and_then(Value::as_str)
                .map(str::to_string),
            assigned_at: value
                .get("assigned_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at: value
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            started_at: value
                .get("started_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            completed_at: value
                .get("completed_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            result: value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string),
            pr_url: value
                .get("pr_url")
                .and_then(Value::as_str)
                .map(str::to_string),
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    fn assign(&mut self, vm_name: &str, session_name: &str) {
        self.assigned_vm = Some(vm_name.to_string());
        self.assigned_session = Some(session_name.to_string());
        self.assigned_at = Some(now_isoformat());
        self.status = TaskStatus::Assigned;
    }

    fn start(&mut self) {
        self.started_at = Some(now_isoformat());
        self.status = TaskStatus::Running;
    }

    fn complete(&mut self, result: &str, pr_url: Option<String>) {
        self.completed_at = Some(now_isoformat());
        self.status = TaskStatus::Completed;
        self.result = Some(result.to_string());
        self.pr_url = pr_url;
    }

    fn fail(&mut self, error: &str) {
        self.completed_at = Some(now_isoformat());
        self.status = TaskStatus::Failed;
        self.error = Some(error.to_string());
    }

    fn requeue(&mut self) {
        self.status = TaskStatus::Queued;
        self.assigned_vm = None;
        self.assigned_session = None;
        self.assigned_at = None;
    }
}

#[derive(Debug, Clone)]
struct TaskQueue {
    tasks: Vec<FleetTask>,
    persist_path: Option<PathBuf>,
    load_failed: bool,
}

impl TaskQueue {
    fn load(persist_path: Option<PathBuf>) -> Result<Self> {
        let mut queue = Self {
            tasks: Vec::new(),
            persist_path,
            load_failed: false,
        };

        let Some(path) = queue.persist_path.clone() else {
            return Ok(queue);
        };
        if !path.exists() {
            return Ok(queue);
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Array(items)) => {
                queue.tasks = items
                    .iter()
                    .filter_map(FleetTask::from_json_value)
                    .collect();
            }
            Ok(_) => {}
            Err(_) => {
                let backup = path.with_extension("json.bak");
                let _ = fs::copy(&path, &backup);
                queue.load_failed = true;
            }
        }

        Ok(queue)
    }

    fn add_task(
        &mut self,
        prompt: &str,
        repo_url: &str,
        priority: TaskPriority,
        agent_command: &str,
        agent_mode: &str,
        max_turns: u32,
    ) -> Result<FleetTask> {
        let task = FleetTask::new(
            prompt,
            repo_url,
            priority,
            agent_command,
            agent_mode,
            max_turns,
        );
        self.tasks.push(task.clone());
        self.save()?;
        Ok(task)
    }

    fn next_task(&self) -> Option<&FleetTask> {
        self.tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Queued)
            .min_by(|left, right| {
                left.priority
                    .rank()
                    .cmp(&right.priority.rank())
                    .then_with(|| left.created_at.cmp(&right.created_at))
            })
    }

    fn active_tasks(&self) -> Vec<&FleetTask> {
        self.tasks
            .iter()
            .filter(|task| matches!(task.status, TaskStatus::Assigned | TaskStatus::Running))
            .collect()
    }

    fn has_active_assignment(&self, vm_name: &str, session_name: &str) -> bool {
        self.tasks.iter().any(|task| {
            matches!(task.status, TaskStatus::Assigned | TaskStatus::Running)
                && task.assigned_vm.as_deref() == Some(vm_name)
                && task.assigned_session.as_deref() == Some(session_name)
        })
    }

    fn summary(&self) -> String {
        let mut lines = vec![format!("Task Queue ({} tasks)", self.tasks.len())];
        for status in [
            TaskStatus::Queued,
            TaskStatus::Assigned,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
        ] {
            let tasks = self
                .tasks
                .iter()
                .filter(|task| task.status == status)
                .collect::<Vec<_>>();
            if tasks.is_empty() {
                continue;
            }

            lines.push(String::new());
            lines.push(format!("  {} ({}):", status.heading(), tasks.len()));
            for task in tasks {
                let vm = task
                    .assigned_vm
                    .as_deref()
                    .map(|vm_name| format!(" -> {vm_name}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "    [{}] {}: {}{}",
                    task.priority.short_label(),
                    task.id,
                    truncate_chars(&task.prompt, 60),
                    vm
                ));
            }
        }

        lines.join("\n")
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        if self.load_failed {
            eprintln!(
                "Refusing to save — load failed for {}. Fix the .bak file manually.",
                path.display()
            );
            return Ok(());
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
                .with_context(|| format!("failed to create temp file for {}", path.display()))?;
        let payload = Value::Array(self.tasks.iter().map(FleetTask::to_json_value).collect());
        let bytes =
            serde_json::to_vec_pretty(&payload).context("failed to serialize task queue")?;
        temp.write_all(&bytes)
            .with_context(|| format!("failed to write {}", path.display()))?;
        temp.persist(path)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to persist {}", path.display()))?;
        Ok(())
    }
}

fn now_isoformat() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.f").to_string()
}

fn render_copilot_status(lock_dir: &Path) -> Result<String> {
    let lock_file = lock_dir.join(".lock_active");
    let goal_file = lock_dir.join(".lock_goal");

    if !lock_file.exists() {
        return Ok("Copilot: not active".to_string());
    }

    if goal_file.exists() {
        let goal_text = fs::read_to_string(&goal_file)
            .with_context(|| format!("failed to read {}", goal_file.display()))?;
        return Ok(format!("Copilot: active\nGoal: {}", goal_text.trim()));
    }

    Ok("Copilot: active (no goal)".to_string())
}

#[derive(Debug, Clone)]
struct CopilotLogReport {
    rendered: String,
    malformed_entries: usize,
}

fn read_copilot_log(log_dir: &Path, tail: usize) -> Result<CopilotLogReport> {
    let decisions_file = log_dir.join("decisions.jsonl");
    if !decisions_file.exists() {
        return Ok(CopilotLogReport {
            rendered: "No decisions recorded.".to_string(),
            malformed_entries: 0,
        });
    }

    let text = fs::read_to_string(&decisions_file)
        .with_context(|| format!("failed to read {}", decisions_file.display()))?;
    if text.trim().is_empty() {
        return Ok(CopilotLogReport {
            rendered: "No decisions recorded.".to_string(),
            malformed_entries: 0,
        });
    }

    let mut malformed_entries = 0usize;
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => entries.push(value),
            Err(_) => malformed_entries += 1,
        }
    }

    if entries.is_empty() {
        return Ok(CopilotLogReport {
            rendered: "No decisions recorded.".to_string(),
            malformed_entries,
        });
    }

    let start = if tail > 0 && entries.len() > tail {
        entries.len() - tail
    } else {
        0
    };

    let mut lines = Vec::new();
    for entry in &entries[start..] {
        let ts = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let action = entry.get("action").and_then(Value::as_str).unwrap_or("?");
        let confidence = value_to_inline_string(entry.get("confidence"));
        lines.push(format!("[{ts}] {action} (confidence={confidence})"));
        let reasoning = entry.get("reasoning").and_then(Value::as_str).unwrap_or("");
        if !reasoning.is_empty() {
            lines.push(format!("  {reasoning}"));
        }
    }

    Ok(CopilotLogReport {
        rendered: lines.join("\n"),
        malformed_entries,
    })
}

fn value_to_inline_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::Null) | None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => other.to_string(),
    }
}

fn render_snapshot(state: &FleetState, observer: &mut FleetObserver) -> Result<String> {
    let managed = state.managed_vms();
    let mut lines = vec![
        format!("Fleet Snapshot ({} managed VMs)", managed.len()),
        "=".repeat(60),
    ];

    for vm in managed.into_iter().filter(|vm| vm.is_running()) {
        lines.push(String::new());
        lines.push(format!("[{}] ({})", vm.name, vm.region));
        if vm.tmux_sessions.is_empty() {
            lines.push("  No sessions".to_string());
            continue;
        }

        for session in &vm.tmux_sessions {
            let observation = observer.observe_session(&vm.name, &session.session_name)?;
            lines.push(format!(
                "  [{}] {}",
                observation.status.as_str(),
                session.session_name
            ));
            for line in observation
                .last_output_lines
                .iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                lines.push(format!("    | {}", truncate_chars(line, 100)));
            }
        }
    }

    Ok(lines.join("\n"))
}

fn render_observe(vm: &VmInfo, observer: &FleetObserver) -> Result<String> {
    let results = observer.observe_all(&vm.tmux_sessions)?;
    let mut lines = Vec::new();
    for observation in results {
        lines.push(String::new());
        lines.push(format!("  Session: {}", observation.session_name));
        lines.push(format!(
            "  Status: {} (confidence: {:.0}%)",
            observation.status.as_str(),
            observation.confidence * 100.0
        ));
        if !observation.matched_pattern.is_empty() {
            lines.push(format!("  Pattern: {}", observation.matched_pattern));
        }
        if !observation.last_output_lines.is_empty() {
            lines.push("  Last output:".to_string());
            for line in observation
                .last_output_lines
                .iter()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                lines.push(format!("    | {}", truncate_chars(line, 120)));
            }
        }
    }

    Ok(lines.join("\n"))
}

fn perceive_fleet_state(azlin_path: PathBuf) -> Result<FleetState> {
    collect_observed_fleet_state(&azlin_path, DEFAULT_CAPTURE_LINES)
}

fn render_report(state: &FleetState, queue: &TaskQueue) -> String {
    [
        "=".repeat(60),
        "Fleet Admiral Report — Cycle 0".to_string(),
        "=".repeat(60),
        String::new(),
        state.summary(),
        String::new(),
        queue.summary(),
        String::new(),
        "Admiral log: 0 actions recorded".to_string(),
        String::new(),
        "Stats: 0 actions, 0 successes, 0 failures".to_string(),
    ]
    .join("\n")
}

#[derive(Debug, Clone)]
struct DryRunSession {
    vm_name: String,
    session_name: String,
}

#[derive(Debug, Clone)]
struct ScoutDiscovery {
    all_vm_count: usize,
    running_vm_count: usize,
    sessions: Vec<DiscoveredSession>,
}

#[derive(Debug, Clone)]
struct DiscoveredSession {
    vm_name: String,
    session_name: String,
    status: AgentStatus,
    cached_tmux_capture: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionAction {
    SendInput,
    Wait,
    Escalate,
    MarkComplete,
    Restart,
}

impl SessionAction {
    fn as_str(self) -> &'static str {
        match self {
            SessionAction::SendInput => "send_input",
            SessionAction::Wait => "wait",
            SessionAction::Escalate => "escalate",
            SessionAction::MarkComplete => "mark_complete",
            SessionAction::Restart => "restart",
        }
    }

    fn next(self) -> Self {
        match self {
            SessionAction::SendInput => SessionAction::Wait,
            SessionAction::Wait => SessionAction::Escalate,
            SessionAction::Escalate => SessionAction::MarkComplete,
            SessionAction::MarkComplete => SessionAction::Restart,
            SessionAction::Restart => SessionAction::SendInput,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionDecision {
    session_name: String,
    vm_name: String,
    action: SessionAction,
    #[serde(default)]
    input_text: String,
    #[serde(default)]
    reasoning: String,
    confidence: f64,
}

impl SessionDecision {
    fn summary(&self) -> String {
        let mut lines = vec![
            format!("  Session: {}/{}", self.vm_name, self.session_name),
            format!("  Action: {}", self.action.as_str()),
            format!("  Confidence: {:.0}%", self.confidence * 100.0),
            format!("  Reasoning: {}", self.reasoning),
        ];
        if !self.input_text.is_empty() {
            lines.push(format!(
                "  Input: \"{}\"",
                truncate_chars(&self.input_text.replace('\n', "\\n"), 100)
            ));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone)]
struct SessionContext {
    vm_name: String,
    session_name: String,
    tmux_capture: String,
    transcript_summary: String,
    working_directory: String,
    git_branch: String,
    repo_url: String,
    agent_status: AgentStatus,
    files_modified: Vec<String>,
    pr_url: String,
    task_prompt: String,
    project_priorities: String,
    health_summary: String,
    project_name: String,
    project_objectives: Vec<ProjectObjective>,
}

impl SessionContext {
    fn new(
        vm_name: &str,
        session_name: &str,
        task_prompt: &str,
        project_priorities: &str,
    ) -> Result<Self> {
        validate_vm_name(vm_name)?;
        validate_session_name(session_name)?;
        Ok(Self {
            vm_name: vm_name.to_string(),
            session_name: session_name.to_string(),
            tmux_capture: String::new(),
            transcript_summary: String::new(),
            working_directory: String::new(),
            git_branch: String::new(),
            repo_url: String::new(),
            agent_status: AgentStatus::Unknown,
            files_modified: Vec::new(),
            pr_url: String::new(),
            task_prompt: task_prompt.to_string(),
            project_priorities: project_priorities.to_string(),
            health_summary: String::new(),
            project_name: String::new(),
            project_objectives: Vec::new(),
        })
    }

    fn to_prompt_context(&self) -> String {
        let mut parts = vec![
            format!("VM: {}, Session: {}", self.vm_name, self.session_name),
            format!("Status: {}", self.agent_status.as_str()),
        ];
        if !self.repo_url.is_empty() {
            parts.push(format!("Repo: {}", self.repo_url));
        }
        if !self.git_branch.is_empty() {
            parts.push(format!("Branch: {}", self.git_branch));
        }
        if !self.task_prompt.is_empty() {
            parts.push(format!("Original task: {}", self.task_prompt));
        }
        if !self.pr_url.is_empty() {
            parts.push(format!("PR: {}", self.pr_url));
        }
        if !self.files_modified.is_empty() {
            parts.push(format!(
                "Files modified: {}",
                self.files_modified.join(", ")
            ));
        }
        if !self.transcript_summary.is_empty() {
            parts.push(format!(
                "\nSession transcript (early + recent messages):\n{}",
                self.transcript_summary
            ));
        }
        parts.push("\nCurrent terminal output (full scrollback):".to_string());
        parts.push(if self.tmux_capture.is_empty() {
            "(empty)".to_string()
        } else {
            self.tmux_capture.clone()
        });
        if !self.health_summary.is_empty() {
            parts.push(format!("\nVM health: {}", self.health_summary));
        }
        if !self.project_name.is_empty() {
            parts.push(format!("\nProject: {}", self.project_name));
            let open = self
                .project_objectives
                .iter()
                .filter(|objective| objective.state == "open")
                .collect::<Vec<_>>();
            if !open.is_empty() {
                parts.push("Open objectives:".to_string());
                for objective in open {
                    parts.push(format!("  - #{}: {}", objective.number, objective.title));
                }
            }
        }
        if !self.project_priorities.is_empty() {
            parts.push(format!("\nProject priorities: {}", self.project_priorities));
        }
        parts.join("\n")
    }

    fn appears_dead(&self) -> bool {
        self.tmux_capture.trim().is_empty() && self.transcript_summary.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
struct SessionAnalysis {
    context: SessionContext,
    decision: SessionDecision,
}

#[derive(Debug, Clone)]
enum NativeReasonerBackend {
    None,
    Claude(PathBuf),
}

impl NativeReasonerBackend {
    fn detect(requested: &str) -> Result<Self> {
        match requested {
            "auto" | "anthropic" | "claude" => Ok(find_reasoner_binary()
                .map(NativeReasonerBackend::Claude)
                .unwrap_or(NativeReasonerBackend::None)),
            "copilot" | "litellm" => bail!(
                "native fleet reasoner backend `{requested}` is not implemented yet; use the default Claude backend"
            ),
            other => bail!("unknown fleet reasoner backend: {other}"),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            NativeReasonerBackend::None => "heuristic",
            NativeReasonerBackend::Claude(_) => "claude",
        }
    }

    fn complete(&self, prompt: &str) -> Result<String> {
        match self {
            NativeReasonerBackend::None => {
                bail!("no native reasoner backend available")
            }
            NativeReasonerBackend::Claude(path) => {
                let mut cmd = Command::new(path);
                cmd.args(["--dangerously-skip-permissions", "-p", prompt]);
                let output = run_output_with_timeout(cmd, SCOUT_REASONER_TIMEOUT)?;
                if !output.status.success() {
                    bail!(
                        "reasoner command failed: {}",
                        truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 200)
                    );
                }
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
        }
    }
}

#[derive(Debug, Clone)]
struct FleetSessionReasoner {
    azlin_path: PathBuf,
    backend: NativeReasonerBackend,
    decisions: Vec<SessionDecision>,
}

impl FleetSessionReasoner {
    fn new(azlin_path: PathBuf, backend: NativeReasonerBackend) -> Self {
        Self {
            azlin_path,
            backend,
            decisions: Vec::new(),
        }
    }

    fn backend_label(&self) -> &'static str {
        self.backend.label()
    }

    fn reason_about_session(
        &mut self,
        vm_name: &str,
        session_name: &str,
        task_prompt: &str,
        project_priorities: &str,
        cached_tmux_capture: Option<&str>,
    ) -> Result<SessionAnalysis> {
        let context = gather_session_context(
            &self.azlin_path,
            vm_name,
            session_name,
            task_prompt,
            project_priorities,
            cached_tmux_capture,
        )?;
        let decision = self.reason(&context);
        self.decisions.push(decision.clone());
        Ok(SessionAnalysis { context, decision })
    }

    fn reason(&self, context: &SessionContext) -> SessionDecision {
        if context.agent_status == AgentStatus::Thinking {
            return SessionDecision {
                session_name: context.session_name.clone(),
                vm_name: context.vm_name.clone(),
                action: SessionAction::Wait,
                input_text: String::new(),
                reasoning: "Agent is actively thinking/processing -- do not interrupt".to_string(),
                confidence: 1.0,
            };
        }
        if context.appears_dead()
            || matches!(
                context.agent_status,
                AgentStatus::Unknown | AgentStatus::NoSession | AgentStatus::Unreachable
            )
        {
            return SessionDecision {
                session_name: context.session_name.clone(),
                vm_name: context.vm_name.clone(),
                action: SessionAction::Wait,
                input_text: String::new(),
                reasoning: "Session is empty or unreachable; no intervention taken".to_string(),
                confidence: CONFIDENCE_UNKNOWN,
            };
        }
        if context.agent_status == AgentStatus::Completed {
            return SessionDecision {
                session_name: context.session_name.clone(),
                vm_name: context.vm_name.clone(),
                action: SessionAction::MarkComplete,
                input_text: String::new(),
                reasoning: "Session output indicates completion".to_string(),
                confidence: CONFIDENCE_COMPLETION,
            };
        }

        if let Ok(response_text) = self.backend.complete(&format!(
            "{}\n\n{}\n\nRespond with JSON only.",
            SESSION_REASONER_SYSTEM_PROMPT,
            context.to_prompt_context()
        )) && let Some(decision) = parse_reasoner_response(&response_text, context)
        {
            return decision;
        }

        heuristic_decision(context)
    }

    fn execute_decision(&self, decision: &SessionDecision) -> Result<()> {
        validate_vm_name(&decision.vm_name)?;
        validate_session_name(&decision.session_name)?;

        match decision.action {
            SessionAction::SendInput => {
                if decision.confidence < MIN_CONFIDENCE_SEND {
                    bail!(
                        "send_input suppressed because confidence {:.2} is below {:.2}",
                        decision.confidence,
                        MIN_CONFIDENCE_SEND
                    );
                }
                if decision.input_text.is_empty() {
                    bail!("send_input requires non-empty input_text");
                }
                if is_dangerous_input(&decision.input_text) {
                    bail!("send_input blocked because it matched the dangerous-input policy");
                }
                let safe_session = shell_single_quote(&decision.session_name);
                for line in decision.input_text.split('\n') {
                    let command = format!(
                        "tmux send-keys -t {safe_session} {} Enter",
                        shell_single_quote(line)
                    );
                    let mut cmd = Command::new(&self.azlin_path);
                    cmd.args([
                        "connect",
                        &decision.vm_name,
                        "--no-tmux",
                        "--yes",
                        "--",
                        &command,
                    ]);
                    let output = run_output_with_timeout(cmd, Duration::from_secs(30))?;
                    if !output.status.success() {
                        bail!(
                            "send_input failed: {}",
                            truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 200)
                        );
                    }
                }
                Ok(())
            }
            SessionAction::Restart => {
                if decision.confidence < MIN_CONFIDENCE_RESTART {
                    bail!(
                        "restart suppressed because confidence {:.2} is below {:.2}",
                        decision.confidence,
                        MIN_CONFIDENCE_RESTART
                    );
                }
                let command = format!(
                    "tmux send-keys -t {} C-c C-c",
                    shell_single_quote(&decision.session_name)
                );
                let mut cmd = Command::new(&self.azlin_path);
                cmd.args([
                    "connect",
                    &decision.vm_name,
                    "--no-tmux",
                    "--yes",
                    "--",
                    &command,
                ]);
                let output = run_output_with_timeout(cmd, Duration::from_secs(30))?;
                if !output.status.success() {
                    bail!(
                        "restart failed: {}",
                        truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 200)
                    );
                }
                Ok(())
            }
            SessionAction::Wait | SessionAction::Escalate | SessionAction::MarkComplete => Ok(()),
        }
    }

    fn dry_run_report(&self) -> String {
        let mut counts = BTreeMap::<String, usize>::new();
        for decision in &self.decisions {
            *counts
                .entry(decision.action.as_str().to_string())
                .or_insert(0) += 1;
        }

        let mut lines = vec![
            format!(
                "Fleet Admiral Dry Run -- {} sessions analyzed",
                self.decisions.len()
            ),
            String::new(),
            "Summary:".to_string(),
        ];
        for (action, count) in counts {
            lines.push(format!("  {action}: {count}"));
        }
        lines.push(String::new());
        for decision in &self.decisions {
            lines.push(decision.summary());
            lines.push(String::new());
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionDecisionRecord {
    vm: String,
    session: String,
    status: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    pr: String,
    action: String,
    confidence: f64,
    reasoning: String,
    #[serde(default)]
    input_text: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    project: String,
    #[serde(default)]
    objectives: Vec<ProjectObjective>,
}

impl SessionDecisionRecord {
    fn from_analysis(analysis: &SessionAnalysis) -> Self {
        Self {
            vm: analysis.context.vm_name.clone(),
            session: analysis.context.session_name.clone(),
            status: analysis.context.agent_status.as_str().to_string(),
            branch: analysis.context.git_branch.clone(),
            pr: analysis.context.pr_url.clone(),
            action: analysis.decision.action.as_str().to_string(),
            confidence: analysis.decision.confidence,
            reasoning: analysis.decision.reasoning.clone(),
            input_text: analysis.decision.input_text.clone(),
            error: None,
            project: analysis.context.project_name.clone(),
            objectives: analysis.context.project_objectives.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SessionExecutionRecord {
    vm: String,
    session: String,
    action: String,
    executed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl SessionExecutionRecord {
    fn executed(record: &SessionDecisionRecord) -> Self {
        Self {
            vm: record.vm.clone(),
            session: record.session.clone(),
            action: record.action.clone(),
            executed: true,
            error: None,
        }
    }

    fn skipped(record: &SessionDecisionRecord, error: Option<String>) -> Self {
        Self {
            vm: record.vm.clone(),
            session: record.session.clone(),
            action: record.action.clone(),
            executed: false,
            error,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct LastScoutSnapshot {
    timestamp: String,
    running_vms: usize,
    total_sessions: usize,
    adopted_count: usize,
    skip_adopt: bool,
    decisions: Vec<SessionDecisionRecord>,
    session_statuses: BTreeMap<String, String>,
}

impl LastScoutSnapshot {
    fn new(
        running_vms: usize,
        total_sessions: usize,
        adopted_count: usize,
        skip_adopt: bool,
        decisions: Vec<SessionDecisionRecord>,
        sessions: &[DiscoveredSession],
    ) -> Self {
        let session_statuses = sessions
            .iter()
            .map(|session| {
                (
                    format!("{}/{}", session.vm_name, session.session_name),
                    session.status.as_str().to_string(),
                )
            })
            .collect();
        Self {
            timestamp: now_isoformat(),
            running_vms,
            total_sessions,
            adopted_count,
            skip_adopt,
            decisions,
            session_statuses,
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        let payload = serde_json::to_value(self).context("failed to serialize scout snapshot")?;
        write_json_file(path, &payload)
    }
}

fn discover_dry_run_sessions(azlin: &Path, vm_names: &[String]) -> Result<Vec<DryRunSession>> {
    let mut state = FleetState::new(azlin.to_path_buf());
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh();

    let target_vms = if vm_names.is_empty() {
        state
            .managed_vms()
            .into_iter()
            .filter(|vm| vm.is_running())
            .map(|vm| vm.name.clone())
            .collect::<Vec<_>>()
    } else {
        vm_names.to_vec()
    };

    if target_vms.is_empty() {
        println!("No managed VMs found. Use 'fleet adopt' to bring VMs under management.");
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for vm in &state.vms {
        if target_vms.iter().any(|name| name == &vm.name) {
            for session in &vm.tmux_sessions {
                sessions.push(DryRunSession {
                    vm_name: vm.name.clone(),
                    session_name: session.session_name.clone(),
                });
            }
        }
    }

    if sessions.is_empty() {
        for vm_name in &target_vms {
            println!("Scanning {vm_name} for sessions...");
            for session in state.poll_tmux_sessions(vm_name) {
                sessions.push(DryRunSession {
                    vm_name: vm_name.clone(),
                    session_name: session.session_name,
                });
            }
        }
    }

    if sessions.is_empty() {
        println!("No sessions found on target VMs.");
    }

    Ok(sessions)
}

fn discover_scout_sessions(
    azlin: &Path,
    vm: Option<&str>,
    session_target: Option<&str>,
    exclude: bool,
) -> Result<Option<ScoutDiscovery>> {
    let mut state = FleetState::new(azlin.to_path_buf());
    if exclude {
        let existing_vms = configured_existing_vms();
        let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
        state.exclude_vms(&existing_refs);
    }
    state.refresh();

    let mut target_vm = vm.map(str::to_string);
    let mut session_filter = None::<String>;
    if let Some(target) = session_target {
        let (vm_name, session_name) = parse_session_target(target);
        if vm_name.is_some() {
            target_vm = vm_name;
        }
        session_filter = Some(session_name);
    }

    let mut all_vms = state.vms.clone();
    if let Some(target) = target_vm.as_deref() {
        all_vms.retain(|candidate| candidate.name == target);
        if all_vms.is_empty() {
            println!("VM not found: {target}");
            return Ok(None);
        }
    }

    let mut running_vms = all_vms
        .iter()
        .filter(|candidate| candidate.is_running() && !candidate.tmux_sessions.is_empty())
        .cloned()
        .collect::<Vec<_>>();

    if let Some(filter) = session_filter.as_deref() {
        for vm_info in &mut running_vms {
            vm_info
                .tmux_sessions
                .retain(|session| session.session_name == filter);
        }
        running_vms.retain(|vm_info| !vm_info.tmux_sessions.is_empty());
        if running_vms.is_empty() {
            println!("Session not found: {}", session_target.unwrap_or(filter));
            return Ok(None);
        }
    }

    let mut observer = FleetObserver::new(azlin.to_path_buf());
    let mut sessions = Vec::<DiscoveredSession>::new();
    for vm_info in &mut running_vms {
        for session in &mut vm_info.tmux_sessions {
            let observation = observer.observe_session(&vm_info.name, &session.session_name)?;
            session.agent_status = observation.status;
            session.last_output = observation.last_output_lines.join("\n");
            sessions.push(DiscoveredSession {
                vm_name: vm_info.name.clone(),
                session_name: session.session_name.clone(),
                status: observation.status,
                cached_tmux_capture: session.last_output.clone(),
            });
        }
    }

    println!(
        "Found {} VMs, {} sessions on {} running VMs",
        all_vms.len(),
        sessions.len(),
        running_vms.len()
    );

    if sessions.is_empty() {
        println!("No running VMs with sessions found.");
        return Ok(None);
    }

    Ok(Some(ScoutDiscovery {
        all_vm_count: all_vms.len(),
        running_vm_count: running_vms.len(),
        sessions,
    }))
}

fn generate_task_id(seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(
        Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .to_string(),
    );
    hasher.update(std::process::id().to_string());
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone)]
struct FleetGraphSummary {
    node_types: Vec<String>,
    edge_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct ProjectInfo {
    repo_url: String,
    name: String,
    github_identity: String,
    priority: String,
    notes: String,
    vms: Vec<String>,
    tasks_total: usize,
    tasks_completed: usize,
    tasks_failed: usize,
    tasks_in_progress: usize,
    prs_created: Vec<String>,
    estimated_cost_usd: f64,
    started_at: Option<String>,
    last_activity: Option<String>,
}

impl ProjectInfo {
    fn new(repo_url: &str, github_identity: &str, name: &str, priority: &str) -> Self {
        let inferred_name = if name.is_empty() {
            repo_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
        } else {
            name
        };

        Self {
            repo_url: repo_url.to_string(),
            name: inferred_name.to_string(),
            github_identity: github_identity.to_string(),
            priority: priority.to_string(),
            notes: String::new(),
            vms: Vec::new(),
            tasks_total: 0,
            tasks_completed: 0,
            tasks_failed: 0,
            tasks_in_progress: 0,
            prs_created: Vec::new(),
            estimated_cost_usd: 0.0,
            started_at: Some(now_isoformat()),
            last_activity: None,
        }
    }

    fn completion_rate(&self) -> f64 {
        if self.tasks_total == 0 {
            0.0
        } else {
            self.tasks_completed as f64 / self.tasks_total as f64
        }
    }

    fn to_json_value(&self) -> Value {
        serde_json::json!({
            "repo_url": self.repo_url,
            "name": self.name,
            "github_identity": self.github_identity,
            "priority": self.priority,
            "notes": self.notes,
            "vms": self.vms,
            "tasks_total": self.tasks_total,
            "tasks_completed": self.tasks_completed,
            "tasks_failed": self.tasks_failed,
            "tasks_in_progress": self.tasks_in_progress,
            "prs_created": self.prs_created,
            "estimated_cost_usd": self.estimated_cost_usd,
            "started_at": self.started_at,
            "last_activity": self.last_activity,
        })
    }

    fn from_json_value(value: &Value) -> Option<Self> {
        Some(Self {
            repo_url: value.get("repo_url")?.as_str()?.to_string(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            github_identity: value
                .get("github_identity")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            priority: value
                .get("priority")
                .and_then(Value::as_str)
                .unwrap_or("medium")
                .to_string(),
            notes: value
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            vms: value
                .get("vms")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            tasks_total: value
                .get("tasks_total")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            tasks_completed: value
                .get("tasks_completed")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            tasks_failed: value
                .get("tasks_failed")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            tasks_in_progress: value
                .get("tasks_in_progress")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            prs_created: value
                .get("prs_created")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            estimated_cost_usd: value
                .get("estimated_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            started_at: value
                .get("started_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            last_activity: value
                .get("last_activity")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

#[derive(Debug, Clone)]
struct FleetDashboardSummary {
    projects: Vec<ProjectInfo>,
    persist_path: Option<PathBuf>,
    load_failed: bool,
}

impl FleetDashboardSummary {
    fn load(persist_path: Option<PathBuf>) -> Result<Self> {
        let mut dashboard = Self {
            projects: Vec::new(),
            persist_path,
            load_failed: false,
        };

        let Some(path) = dashboard.persist_path.clone() else {
            return Ok(dashboard);
        };
        if !path.exists() {
            return Ok(dashboard);
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Array(items)) => {
                dashboard.projects = items
                    .iter()
                    .filter_map(ProjectInfo::from_json_value)
                    .collect();
            }
            Ok(_) => {}
            Err(_) => {
                let backup = path.with_extension("json.bak");
                let _ = fs::copy(&path, &backup);
                dashboard.load_failed = true;
            }
        }

        Ok(dashboard)
    }

    fn add_project(
        &mut self,
        repo_url: &str,
        github_identity: &str,
        name: &str,
        priority: &str,
    ) -> usize {
        if let Some(index) = self
            .projects
            .iter()
            .position(|project| project.repo_url == repo_url || project.name == name)
        {
            return index;
        }

        self.projects
            .push(ProjectInfo::new(repo_url, github_identity, name, priority));
        self.projects.len() - 1
    }

    fn get_project(&self, name_or_url: &str) -> Option<&ProjectInfo> {
        self.projects
            .iter()
            .find(|project| project.name == name_or_url || project.repo_url == name_or_url)
    }

    fn remove_project(&mut self, name_or_url: &str) -> bool {
        let Some(index) = self
            .projects
            .iter()
            .position(|project| project.name == name_or_url || project.repo_url == name_or_url)
        else {
            return false;
        };

        self.projects.remove(index);
        true
    }

    fn update_from_queue(&mut self, queue: &TaskQueue) -> Result<()> {
        let mut grouped = std::collections::BTreeMap::<String, Vec<&FleetTask>>::new();
        for task in &queue.tasks {
            let key = if task.repo_url.is_empty() {
                "unassigned".to_string()
            } else {
                task.repo_url.clone()
            };
            grouped.entry(key).or_default().push(task);
        }

        for (repo_url, tasks) in grouped {
            if repo_url == "unassigned" {
                continue;
            }
            let index = self.add_project(&repo_url, "", "", "medium");
            let project = &mut self.projects[index];
            project.tasks_total = tasks.len();
            project.tasks_completed = tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Completed)
                .count();
            project.tasks_failed = tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Failed)
                .count();
            project.tasks_in_progress = tasks
                .iter()
                .filter(|task| matches!(task.status, TaskStatus::Assigned | TaskStatus::Running))
                .count();
            project.prs_created = tasks
                .iter()
                .filter_map(|task| task.pr_url.clone())
                .collect();
            project.vms = tasks
                .iter()
                .filter_map(|task| task.assigned_vm.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            project.last_activity = Some(now_isoformat());
        }

        self.save()
    }

    fn summary(&self) -> String {
        let total_tasks = self
            .projects
            .iter()
            .map(|project| project.tasks_total)
            .sum::<usize>();
        let total_completed = self
            .projects
            .iter()
            .map(|project| project.tasks_completed)
            .sum::<usize>();
        let total_prs = self
            .projects
            .iter()
            .map(|project| project.prs_created.len())
            .sum::<usize>();
        let total_cost = self
            .projects
            .iter()
            .map(|project| project.estimated_cost_usd)
            .sum::<f64>();
        let total_vms = self
            .projects
            .iter()
            .flat_map(|project| project.vms.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .len();

        let mut lines = vec![
            "=".repeat(60),
            "FLEET DASHBOARD".to_string(),
            "=".repeat(60),
            format!("  Projects: {}", self.projects.len()),
            format!("  VMs in use: {total_vms}"),
            format!("  Tasks: {total_completed}/{total_tasks} completed"),
            format!("  PRs created: {total_prs}"),
            format!("  Estimated cost: ${total_cost:.2}"),
            String::new(),
        ];

        for project in &self.projects {
            let identity = if project.github_identity.is_empty() {
                String::new()
            } else {
                format!(" ({})", project.github_identity)
            };
            lines.push(format!("  [{}]{}", project.name, identity));
            lines.push(format!(
                "    {} {}/{} tasks",
                Self::progress_bar(project.completion_rate(), 20),
                project.tasks_completed,
                project.tasks_total
            ));
            lines.push(format!(
                "    VMs: {} | PRs: {} | Cost: ${:.2}",
                if project.vms.is_empty() {
                    "none".to_string()
                } else {
                    project.vms.join(", ")
                },
                project.prs_created.len(),
                project.estimated_cost_usd
            ));
            if project.tasks_failed > 0 {
                lines.push(format!("    !! {} failed tasks", project.tasks_failed));
            }
            lines.push(String::new());
        }

        lines.join("\n")
    }

    fn progress_bar(ratio: f64, width: usize) -> String {
        let filled = (width as f64 * ratio).floor() as usize;
        let bar = "#".repeat(filled) + &"-".repeat(width.saturating_sub(filled));
        let pct = (ratio * 100.0).floor() as usize;
        format!("[{bar}] {pct}%")
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        if self.load_failed {
            eprintln!(
                "Refusing to save — load failed for {}. Fix the .bak file manually.",
                path.display()
            );
            return Ok(());
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
                .with_context(|| format!("failed to create temp file for {}", path.display()))?;
        let payload = Value::Array(
            self.projects
                .iter()
                .map(ProjectInfo::to_json_value)
                .collect(),
        );
        let bytes =
            serde_json::to_vec_pretty(&payload).context("failed to serialize fleet dashboard")?;
        temp.write_all(&bytes)
            .with_context(|| format!("failed to write {}", path.display()))?;
        temp.persist(path)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to persist {}", path.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectRegistryDoc {
    #[serde(default)]
    project: BTreeMap<String, ProjectRegistryEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectRegistryEntry {
    #[serde(default)]
    repo_url: String,
    #[serde(default)]
    identity: String,
    #[serde(default = "default_project_priority")]
    priority: String,
    #[serde(default)]
    objectives: Vec<ProjectObjective>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectObjective {
    number: i64,
    title: String,
    #[serde(default = "default_objective_state")]
    state: String,
    #[serde(default)]
    url: String,
}

fn default_project_priority() -> String {
    "medium".to_string()
}

fn default_objective_state() -> String {
    "open".to_string()
}

fn load_projects_registry(path: &Path) -> Result<BTreeMap<String, ProjectRegistryEntry>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let doc = toml::from_str::<ProjectRegistryDoc>(&raw).unwrap_or_default();
    Ok(doc.project)
}

fn save_projects_registry(
    projects: &BTreeMap<String, ProjectRegistryEntry>,
    path: &Path,
) -> Result<()> {
    for name in projects.keys() {
        validate_project_name(name)?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let doc = ProjectRegistryDoc {
        project: projects.clone(),
    };
    let rendered = toml::to_string(&doc).context("failed to serialize project registry")?;
    fs::write(path, rendered).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn validate_project_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Invalid project name {name:?}: must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$");
    };
    if !first.is_ascii_alphanumeric()
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        bail!("Invalid project name {name:?}: must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$");
    }
    Ok(())
}

fn render_project_list(dashboard: &FleetDashboardSummary) -> String {
    if dashboard.projects.is_empty() {
        return "No projects registered. Use 'fleet project add <repo_url>' to add one."
            .to_string();
    }

    let mut lines = vec![
        format!("Fleet Projects ({})", dashboard.projects.len()),
        "=".repeat(60),
    ];
    for project in &dashboard.projects {
        let prio_label = match project.priority.as_str() {
            "high" => "!!!",
            "low" => "!",
            _ => "!!",
        };
        lines.push(format!("  [{prio_label}] {}", project.name));
        lines.push(format!("      Repo: {}", project.repo_url));
        if !project.github_identity.is_empty() {
            lines.push(format!("      Identity: {}", project.github_identity));
        }
        lines.push(format!("      Priority: {}", project.priority));
        lines.push(format!(
            "      VMs: {} | Tasks: {}/{} | PRs: {}",
            project.vms.len(),
            project.tasks_completed,
            project.tasks_total,
            project.prs_created.len()
        ));
        if !project.notes.is_empty() {
            lines.push(format!("      Notes: {}", project.notes));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

impl FleetGraphSummary {
    fn load(persist_path: Option<PathBuf>) -> Result<Self> {
        let Some(path) = persist_path else {
            return Ok(Self {
                node_types: Vec::new(),
                edge_types: Vec::new(),
            });
        };
        if !path.exists() {
            return Ok(Self {
                node_types: Vec::new(),
                edge_types: Vec::new(),
            });
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let value = match serde_json::from_str::<Value>(&raw) {
            Ok(value) => value,
            Err(_) => {
                let backup = path.with_extension("json.bak");
                let _ = fs::copy(&path, &backup);
                return Ok(Self {
                    node_types: Vec::new(),
                    edge_types: Vec::new(),
                });
            }
        };

        let node_types = value
            .get("nodes")
            .and_then(Value::as_object)
            .map(|nodes| {
                nodes
                    .values()
                    .filter_map(|node| node.get("type").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let edge_types = value
            .get("edges")
            .and_then(Value::as_array)
            .map(|edges| {
                edges
                    .iter()
                    .filter_map(|edge| edge.get("type").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Self {
            node_types,
            edge_types,
        })
    }

    fn summary(&self) -> String {
        let mut node_counts = std::collections::BTreeMap::<String, usize>::new();
        for node_type in &self.node_types {
            *node_counts.entry(node_type.clone()).or_insert(0) += 1;
        }
        let mut edge_counts = std::collections::BTreeMap::<String, usize>::new();
        for edge_type in &self.edge_types {
            *edge_counts.entry(edge_type.clone()).or_insert(0) += 1;
        }

        let mut lines = vec![
            format!(
                "Fleet Graph: {} nodes, {} edges",
                self.node_types.len(),
                self.edge_types.len()
            ),
            format!(
                "  Nodes: {}",
                node_counts
                    .iter()
                    .map(|(kind, count)| format!("{kind}={count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            format!(
                "  Edges: {}",
                edge_counts
                    .iter()
                    .map(|(kind, count)| format!("{kind}={count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ];

        let conflicts = edge_counts.get("conflicts").copied().unwrap_or(0);
        if conflicts > 0 {
            lines.push(format!("  !! {conflicts} conflicts detected"));
        }

        lines.join("\n")
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentStatus {
    Unknown,
    Thinking,
    Running,
    Idle,
    Shell,
    NoSession,
    Unreachable,
    Completed,
    Stuck,
    Error,
    WaitingInput,
}

impl AgentStatus {
    fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Unknown => "unknown",
            AgentStatus::Thinking => "thinking",
            AgentStatus::Running => "running",
            AgentStatus::Idle => "idle",
            AgentStatus::Shell => "shell",
            AgentStatus::NoSession => "no_session",
            AgentStatus::Unreachable => "unreachable",
            AgentStatus::Completed => "completed",
            AgentStatus::Stuck => "stuck",
            AgentStatus::Error => "error",
            AgentStatus::WaitingInput => "waiting_input",
        }
    }

    fn summary_icon(self) -> char {
        match self {
            AgentStatus::Thinking => '*',
            AgentStatus::Running => '>',
            AgentStatus::Completed => '=',
            AgentStatus::Stuck => '!',
            AgentStatus::Error => 'X',
            AgentStatus::Idle => '~',
            AgentStatus::Shell => '$',
            AgentStatus::NoSession => '0',
            AgentStatus::Unreachable => 'U',
            AgentStatus::WaitingInput => '?',
            AgentStatus::Unknown => '.',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TmuxSessionInfo {
    session_name: String,
    vm_name: String,
    windows: u32,
    attached: bool,
    agent_status: AgentStatus,
    last_output: String,
    working_directory: String,
    repo_url: String,
    git_branch: String,
    pr_url: String,
    task_summary: String,
}

#[derive(Debug, Clone)]
struct ObservationResult {
    session_name: String,
    status: AgentStatus,
    last_output_lines: Vec<String>,
    confidence: f64,
    matched_pattern: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct AuthResult {
    service: String,
    vm_name: String,
    success: bool,
    files_copied: Vec<String>,
    error: Option<String>,
    duration_seconds: f64,
}

#[derive(Debug, Clone)]
struct AuthPropagator {
    azlin_path: PathBuf,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionType {
    StartAgent,
    StopAgent,
    ReassignTask,
    MarkComplete,
    MarkFailed,
    Report,
    PropagateAuth,
}

impl ActionType {
    fn as_str(self) -> &'static str {
        match self {
            ActionType::StartAgent => "start_agent",
            ActionType::StopAgent => "stop_agent",
            ActionType::ReassignTask => "reassign_task",
            ActionType::MarkComplete => "mark_complete",
            ActionType::MarkFailed => "mark_failed",
            ActionType::Report => "report",
            ActionType::PropagateAuth => "propagate_auth",
        }
    }
}

#[derive(Debug, Clone)]
struct DirectorAction {
    action_type: ActionType,
    task: Option<FleetTask>,
    vm_name: Option<String>,
    session_name: Option<String>,
    reason: String,
    timestamp: String,
}

impl DirectorAction {
    fn new(
        action_type: ActionType,
        task: Option<FleetTask>,
        vm_name: Option<String>,
        session_name: Option<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            action_type,
            task,
            vm_name,
            session_name,
            reason: reason.into(),
            timestamp: now_isoformat(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct DirectorLog {
    actions: Vec<Value>,
    persist_path: Option<PathBuf>,
}

impl DirectorLog {
    fn record(&mut self, action: &DirectorAction, outcome: &str) -> Result<()> {
        self.actions.push(serde_json::json!({
            "timestamp": action.timestamp,
            "action": action.action_type.as_str(),
            "vm": action.vm_name,
            "session": action.session_name,
            "task_id": action.task.as_ref().map(|task| task.id.clone()),
            "reason": action.reason,
            "outcome": outcome,
        }));
        self.save()
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.actions)
            .context("failed to serialize admiral action log")?;
        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
                .with_context(|| format!("failed to create temp file for {}", path.display()))?;
        temp.write_all(&bytes)
            .with_context(|| format!("failed to write {}", path.display()))?;
        temp.persist(path)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to persist {}", path.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct AdmiralStats {
    actions: usize,
    successes: usize,
    failures: usize,
}

#[derive(Debug, Clone)]
struct FleetAdmiral {
    task_queue: TaskQueue,
    azlin_path: PathBuf,
    poll_interval_seconds: u64,
    max_agents_per_vm: usize,
    fleet_state: FleetState,
    observer: FleetObserver,
    auth: AuthPropagator,
    log: DirectorLog,
    exclude_vms: Vec<String>,
    cycle_count: usize,
    missing_session_counts: BTreeMap<String, usize>,
    stats: AdmiralStats,
    coordination_dir: PathBuf,
}

impl FleetAdmiral {
    fn new(azlin_path: PathBuf, task_queue: TaskQueue, log_dir: Option<PathBuf>) -> Result<Self> {
        if let Some(dir) = &log_dir {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let log = DirectorLog {
            persist_path: log_dir.map(|dir| dir.join("admiral_log.json")),
            ..DirectorLog::default()
        };
        Ok(Self {
            task_queue,
            fleet_state: FleetState::new(azlin_path.clone()),
            observer: FleetObserver::new(azlin_path.clone()),
            auth: AuthPropagator::new(azlin_path.clone()),
            azlin_path,
            poll_interval_seconds: DEFAULT_POLL_INTERVAL_SECONDS,
            max_agents_per_vm: DEFAULT_MAX_AGENTS_PER_VM,
            log,
            exclude_vms: Vec::new(),
            cycle_count: 0,
            missing_session_counts: BTreeMap::new(),
            stats: AdmiralStats::default(),
            coordination_dir: default_coordination_dir(),
        })
    }

    fn exclude_vms(&mut self, vm_names: &[&str]) {
        self.exclude_vms
            .extend(vm_names.iter().map(|name| (*name).to_string()));
        self.fleet_state.exclude_vms(vm_names);
    }

    fn run_once(&mut self) -> Result<Vec<DirectorAction>> {
        self.cycle_count += 1;
        self.perceive()?;
        let actions = self.reason()?;
        let results = self.act(&actions)?;
        self.learn(&results);
        Ok(actions)
    }

    fn run_loop(&mut self, max_cycles: u32) -> Result<()> {
        let mut cycle = 0u32;
        let mut consecutive_failures = 0usize;

        loop {
            cycle += 1;
            if max_cycles > 0 && cycle > max_cycles {
                break;
            }

            match self.run_once() {
                Ok(_) => consecutive_failures = 0,
                Err(error) => {
                    consecutive_failures += 1;
                    eprintln!(
                        "Admiral cycle error ({}/5): {}",
                        consecutive_failures, error
                    );
                    if consecutive_failures >= 5 {
                        eprintln!("CIRCUIT BREAKER: 5 consecutive failures. Stopping admiral.");
                        break;
                    }
                }
            }

            if self.task_queue.next_task().is_none() && self.task_queue.active_tasks().is_empty() {
                break;
            }

            thread::sleep(Duration::from_secs(self.poll_interval_seconds));
        }

        Ok(())
    }

    fn perceive(&mut self) -> Result<()> {
        self.fleet_state.refresh();
        let excluded = self.exclude_vms.clone();
        for vm in &mut self.fleet_state.vms {
            if !vm.is_running() || excluded.iter().any(|name| name == &vm.name) {
                continue;
            }
            for session in &mut vm.tmux_sessions {
                let observation = self
                    .observer
                    .observe_session(&vm.name, &session.session_name)?;
                session.agent_status = observation.status;
                session.last_output = observation.last_output_lines.join("\n");
            }
        }
        Ok(())
    }

    fn reason(&mut self) -> Result<Vec<DirectorAction>> {
        self.write_coordination_files()?;

        let mut actions = Vec::new();
        actions.extend(self.lifecycle_actions());
        actions.extend(self.preemption_actions());
        actions.extend(self.batch_assign_actions(&actions));
        self.task_queue.save()?;
        Ok(actions)
    }

    fn lifecycle_actions(&mut self) -> Vec<DirectorAction> {
        let active_keys = self
            .task_queue
            .active_tasks()
            .iter()
            .filter_map(|task| {
                Some(format!(
                    "{}:{}",
                    task.assigned_vm.as_deref()?,
                    task.assigned_session.as_deref()?
                ))
            })
            .collect::<Vec<_>>();
        self.missing_session_counts
            .retain(|key, _| active_keys.iter().any(|active| active == key));

        let mut actions = Vec::new();
        let active_tasks = self
            .task_queue
            .active_tasks()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        for task in active_tasks {
            let (Some(vm_name), Some(session_name)) =
                (task.assigned_vm.clone(), task.assigned_session.clone())
            else {
                continue;
            };

            let Some(vm) = self.fleet_state.get_vm(&vm_name) else {
                continue;
            };
            let session = vm
                .tmux_sessions
                .iter()
                .find(|candidate| candidate.session_name == session_name);
            if let Some(session) = session {
                let key = format!("{vm_name}:{session_name}");
                self.missing_session_counts.remove(&key);
                match session.agent_status {
                    AgentStatus::Completed => actions.push(DirectorAction::new(
                        ActionType::MarkComplete,
                        Some(task.clone()),
                        Some(vm_name),
                        Some(session_name),
                        "Agent completed successfully",
                    )),
                    AgentStatus::Error | AgentStatus::Shell | AgentStatus::NoSession => actions
                        .push(DirectorAction::new(
                            ActionType::MarkFailed,
                            Some(task.clone()),
                            Some(vm_name),
                            Some(session_name),
                            format!("Agent error: {}", truncate_chars(&session.last_output, 200)),
                        )),
                    AgentStatus::Stuck if !task.protected => actions.push(DirectorAction::new(
                        ActionType::ReassignTask,
                        Some(task.clone()),
                        Some(vm_name),
                        Some(session_name),
                        "Agent appears stuck",
                    )),
                    _ => {}
                }
                continue;
            }

            let key = format!("{vm_name}:{session_name}");
            let next_count = self.missing_session_counts.get(&key).copied().unwrap_or(0) + 1;
            if next_count >= 2 {
                self.missing_session_counts.remove(&key);
                actions.push(DirectorAction::new(
                    ActionType::MarkFailed,
                    Some(task),
                    Some(vm_name),
                    Some(session_name),
                    "Session no longer exists (missing 2+ cycles)",
                ));
            } else {
                self.missing_session_counts.insert(key, next_count);
            }
        }

        actions
    }

    fn preemption_actions(&self) -> Vec<DirectorAction> {
        let critical_queued = self
            .task_queue
            .tasks
            .iter()
            .filter(|task| {
                task.status == TaskStatus::Queued && task.priority == TaskPriority::Critical
            })
            .cloned()
            .collect::<Vec<_>>();
        if critical_queued.is_empty() || !self.fleet_state.idle_vms().is_empty() {
            return Vec::new();
        }

        let mut running = self
            .task_queue
            .active_tasks()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        running.sort_by(|left, right| right.priority.rank().cmp(&left.priority.rank()));

        let mut actions = Vec::new();
        for critical_task in critical_queued {
            if running.is_empty() {
                break;
            }
            let victim = running.remove(0);
            if victim.priority.rank() <= critical_task.priority.rank() {
                break;
            }
            if victim.protected {
                continue;
            }
            actions.push(DirectorAction::new(
                ActionType::ReassignTask,
                Some(victim.clone()),
                victim.assigned_vm.clone(),
                victim.assigned_session.clone(),
                format!("Preempted for CRITICAL task {}", critical_task.id),
            ));
        }

        actions
    }

    fn batch_assign_actions(&self, prior_actions: &[DirectorAction]) -> Vec<DirectorAction> {
        let mut queued = self
            .task_queue
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Queued)
            .cloned()
            .collect::<Vec<_>>();
        queued.sort_by(|left, right| {
            left.priority
                .rank()
                .cmp(&right.priority.rank())
                .then_with(|| left.created_at.cmp(&right.created_at))
        });
        if queued.is_empty() {
            return Vec::new();
        }

        let mut capacity = BTreeMap::<String, usize>::new();
        for vm in self.fleet_state.managed_vms() {
            if !vm.is_running() {
                continue;
            }
            let mut used = vm.active_agents();
            used += prior_actions
                .iter()
                .filter(|action| {
                    action.action_type == ActionType::StartAgent
                        && action.vm_name.as_deref() == Some(vm.name.as_str())
                })
                .count();
            if self.max_agents_per_vm > used {
                capacity.insert(vm.name.clone(), self.max_agents_per_vm - used);
            }
        }
        if capacity.is_empty() {
            return Vec::new();
        }

        let mut actions = Vec::new();
        for task in queued {
            let Some((best_vm, remaining)) = capacity
                .iter()
                .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
                .map(|(name, remaining)| (name.clone(), *remaining))
            else {
                break;
            };

            actions.push(DirectorAction::new(
                ActionType::StartAgent,
                Some(task.clone()),
                Some(best_vm.clone()),
                Some(format!("fleet-{}", task.id)),
                format!("Batch assign: {} task", task.priority.as_name()),
            ));

            if remaining <= 1 {
                capacity.remove(&best_vm);
            } else {
                capacity.insert(best_vm, remaining - 1);
            }
        }

        actions
    }

    fn act(&mut self, actions: &[DirectorAction]) -> Result<Vec<(DirectorAction, String)>> {
        let mut results = Vec::new();
        for action in actions {
            let outcome = match self.execute_action(action) {
                Ok(outcome) => outcome,
                Err(error) => format!("ERROR: {error}"),
            };
            self.log.record(action, &outcome)?;
            results.push((action.clone(), outcome));
        }
        Ok(results)
    }

    fn execute_action(&mut self, action: &DirectorAction) -> Result<String> {
        match action.action_type {
            ActionType::StartAgent => self.start_agent(action),
            ActionType::MarkComplete => self.mark_complete(action),
            ActionType::MarkFailed => self.mark_failed(action),
            ActionType::ReassignTask => self.reassign_task(action),
            ActionType::PropagateAuth => self.propagate_auth(action),
            ActionType::StopAgent | ActionType::Report => {
                Ok(format!("Unknown action: {}", action.action_type.as_str()))
            }
        }
    }

    fn start_agent(&mut self, action: &DirectorAction) -> Result<String> {
        let Some(task) = action.task.as_ref() else {
            return Ok("ERROR: No task provided".to_string());
        };
        let Some(vm_name) = action.vm_name.as_deref() else {
            return Ok("ERROR: No VM name provided".to_string());
        };
        let session_name = action.session_name.as_deref().unwrap_or("fleet-session");
        validate_vm_name(vm_name)?;
        validate_session_name(session_name)?;

        if !matches!(
            task.agent_command.as_str(),
            "claude" | "amplifier" | "copilot"
        ) {
            return Ok(format!(
                "ERROR: Invalid agent command: {:?}",
                task.agent_command
            ));
        }
        if !matches!(task.agent_mode.as_str(), "auto" | "ultrathink") {
            return Ok(format!("ERROR: Invalid agent mode: {:?}", task.agent_mode));
        }
        if task.max_turns == 0 || task.max_turns > 1000 {
            return Ok(format!("ERROR: Invalid max_turns: {:?}", task.max_turns));
        }

        let setup_cmd = format!(
            "tmux new-session -d -s {} && tmux send-keys -t {} 'amplihack {} --{} --max-turns {} -- -p {}' C-m",
            shell_single_quote(session_name),
            shell_single_quote(session_name),
            shell_single_quote(&task.agent_command),
            shell_single_quote(&task.agent_mode),
            shell_single_quote(&task.max_turns.to_string()),
            shell_single_quote(&task.prompt),
        );

        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", &setup_cmd]);
        let output = run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT)?;
        if output.status.success() {
            if let Some(saved_task) = self
                .task_queue
                .tasks
                .iter_mut()
                .find(|candidate| candidate.id == task.id)
            {
                saved_task.assign(vm_name, session_name);
                saved_task.start();
            }
            self.task_queue.save()?;
            return Ok(format!("Agent started: {} on {}", session_name, vm_name));
        }

        Ok(format!(
            "ERROR: Failed to start agent: {}",
            truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 200)
        ))
    }

    fn mark_complete(&mut self, action: &DirectorAction) -> Result<String> {
        if let Some(task) = action.task.as_ref() {
            if let Some(saved_task) = self
                .task_queue
                .tasks
                .iter_mut()
                .find(|candidate| candidate.id == task.id)
            {
                saved_task.complete("Detected as completed by observer", None);
            }
            self.task_queue.save()?;
        }
        Ok("Task marked complete".to_string())
    }

    fn mark_failed(&mut self, action: &DirectorAction) -> Result<String> {
        if let Some(task) = action.task.as_ref() {
            if let Some(saved_task) = self
                .task_queue
                .tasks
                .iter_mut()
                .find(|candidate| candidate.id == task.id)
            {
                saved_task.fail(&action.reason);
            }
            self.task_queue.save()?;
        }
        Ok(format!("Task marked failed: {}", action.reason))
    }

    fn reassign_task(&mut self, action: &DirectorAction) -> Result<String> {
        let (Some(task), Some(vm_name), Some(session_name)) = (
            action.task.as_ref(),
            action.vm_name.as_deref(),
            action.session_name.as_deref(),
        ) else {
            return Ok("ERROR: Missing task/vm/session for reassignment".to_string());
        };
        validate_vm_name(vm_name)?;
        validate_session_name(session_name)?;

        let kill_cmd = format!(
            "tmux kill-session -t {} 2>/dev/null || true",
            shell_single_quote(session_name)
        );
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", &kill_cmd]);
        let _ = run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT);

        if let Some(saved_task) = self
            .task_queue
            .tasks
            .iter_mut()
            .find(|candidate| candidate.id == task.id)
        {
            saved_task.requeue();
        }
        self.task_queue.save()?;
        Ok("Stuck agent killed, task requeued".to_string())
    }

    fn propagate_auth(&mut self, action: &DirectorAction) -> Result<String> {
        let Some(vm_name) = action.vm_name.as_deref() else {
            return Ok("ERROR: No VM specified".to_string());
        };
        let results = self
            .auth
            .propagate_all(vm_name, &["github".into(), "azure".into(), "claude".into()]);
        let success = results.iter().filter(|result| result.success).count();
        Ok(format!(
            "Auth propagated: {success}/{} services",
            results.len()
        ))
    }

    fn learn(&mut self, results: &[(DirectorAction, String)]) {
        for (_action, outcome) in results {
            self.stats.actions += 1;
            if outcome.starts_with("ERROR") {
                self.stats.failures += 1;
            } else {
                self.stats.successes += 1;
            }
        }
    }

    fn adopt_all_sessions(&mut self) -> Result<usize> {
        self.fleet_state.refresh();
        let adopter = SessionAdopter::new(self.azlin_path.clone());
        let mut total = 0usize;
        for vm in self.fleet_state.managed_vms() {
            if !vm.is_running() {
                continue;
            }
            total += adopter
                .adopt_sessions(&vm.name, &mut self.task_queue, None)?
                .len();
        }
        Ok(total)
    }

    fn write_coordination_files(&self) -> Result<()> {
        let mut grouped = BTreeMap::<String, Vec<&FleetTask>>::new();
        for task in self.task_queue.active_tasks() {
            if task.repo_url.is_empty() {
                continue;
            }
            grouped.entry(task.repo_url.clone()).or_default().push(task);
        }

        fs::create_dir_all(&self.coordination_dir)
            .with_context(|| format!("failed to create {}", self.coordination_dir.display()))?;
        for (repo_url, tasks) in grouped {
            if tasks.len() < 2 {
                continue;
            }
            let safe_key = repo_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("coordination")
                .trim_end_matches(".git");
            let path = self.coordination_dir.join(format!("{safe_key}.json"));
            let payload = serde_json::json!({
                "repo": repo_url,
                "active_agents": tasks.iter().map(|task| serde_json::json!({
                    "task_id": task.id,
                    "prompt": task.prompt,
                    "vm": task.assigned_vm,
                    "session": task.assigned_session,
                })).collect::<Vec<_>>(),
                "updated_at": now_isoformat(),
            });
            let bytes = serde_json::to_vec_pretty(&payload)
                .context("failed to serialize coordination file")?;
            let mut temp =
                tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
                    .with_context(|| {
                        format!("failed to create temp file for {}", path.display())
                    })?;
            temp.write_all(&bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
            temp.persist(&path)
                .map_err(|err| err.error)
                .with_context(|| format!("failed to persist {}", path.display()))?;
        }
        Ok(())
    }
}

impl AuthPropagator {
    fn new(azlin_path: PathBuf) -> Self {
        Self { azlin_path }
    }

    fn propagate_all(&self, vm_name: &str, services: &[String]) -> Vec<AuthResult> {
        let target_services = if services.is_empty() {
            vec![
                "github".to_string(),
                "azure".to_string(),
                "claude".to_string(),
            ]
        } else {
            services.to_vec()
        };

        target_services
            .into_iter()
            .map(|service| {
                if auth_files_for_service(&service).is_none() {
                    return AuthResult {
                        service: service.clone(),
                        vm_name: vm_name.to_string(),
                        success: false,
                        files_copied: Vec::new(),
                        error: Some(format!("Unknown service: {}", service)),
                        duration_seconds: 0.0,
                    };
                }

                self.propagate_service(vm_name, &service)
            })
            .collect()
    }

    fn verify_auth(&self, vm_name: &str) -> Vec<(String, bool)> {
        let checks = [
            ("github", "gh auth status"),
            ("azure", "az account show --query name -o tsv"),
        ];

        checks
            .into_iter()
            .map(|(service, command)| {
                let works = self
                    .remote_exec(vm_name, command)
                    .map(|output| output.status.success())
                    .unwrap_or(false);
                (service.to_string(), works)
            })
            .collect()
    }

    fn propagate_service(&self, vm_name: &str, service: &str) -> AuthResult {
        let start = std::time::Instant::now();
        let mut files_copied = Vec::new();
        let mut errors = Vec::new();
        let Some(files) = auth_files_for_service(service) else {
            return AuthResult {
                service: service.to_string(),
                vm_name: vm_name.to_string(),
                success: false,
                files_copied,
                error: Some(format!("Unknown service: {}", service)),
                duration_seconds: 0.0,
            };
        };

        let mut dest_dirs = Vec::<String>::new();
        for (_, dest, _) in files {
            let parent = remote_parent_dir(dest);
            if !dest_dirs.iter().any(|existing| existing == &parent) {
                dest_dirs.push(parent);
            }
        }
        for dest_dir in dest_dirs {
            let command = format!("mkdir -p {}", shell_single_quote(&dest_dir));
            let _ = self.remote_exec(vm_name, &command);
        }

        for (src, dest, mode) in files {
            let src_path = expand_tilde(src);
            if !src_path.exists() {
                continue;
            }

            let mut cmd = Command::new(&self.azlin_path);
            cmd.args([
                "cp",
                &src_path.to_string_lossy(),
                &format!("{vm_name}:{dest}"),
            ]);
            match run_output_with_timeout(cmd, Duration::from_secs(60)) {
                Ok(output) if output.status.success() => {
                    if validate_chmod_mode(mode).is_ok() {
                        let chmod = format!("chmod {mode} {}", shell_single_quote(dest));
                        let _ = self.remote_exec(vm_name, &chmod);
                    }
                    if let Some(name) = src_path.file_name().and_then(|name| name.to_str()) {
                        files_copied.push(name.to_string());
                    }
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let file_name = src_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("file");
                    errors.push(format!("Failed to copy {file_name}: {}", stderr.trim()));
                }
                Err(error) => {
                    let file_name = src_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("file");
                    let message = error.to_string();
                    if message.contains("timed out after") {
                        errors.push(format!("Timeout copying {file_name}"));
                    } else {
                        errors.push(format!("Error copying {file_name}: {message}"));
                    }
                }
            }
        }

        AuthResult {
            service: service.to_string(),
            vm_name: vm_name.to_string(),
            success: errors.is_empty(),
            files_copied,
            error: (!errors.is_empty()).then(|| errors.join("; ")),
            duration_seconds: start.elapsed().as_secs_f64(),
        }
    }

    fn remote_exec(&self, vm_name: &str, command: &str) -> Result<Output> {
        validate_vm_name(vm_name)?;
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", command]);
        run_output_with_timeout(cmd, Duration::from_secs(30))
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
struct AdoptedSession {
    vm_name: String,
    session_name: String,
    inferred_repo: String,
    inferred_branch: String,
    inferred_task: String,
    inferred_pr: String,
    working_directory: String,
    agent_type: String,
    adopted_at: Option<String>,
    task_id: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionAdopter {
    azlin_path: PathBuf,
}

impl SessionAdopter {
    fn new(azlin_path: PathBuf) -> Self {
        Self { azlin_path }
    }

    fn discover_sessions(&self, vm_name: &str) -> Vec<AdoptedSession> {
        let discover_command = self.build_discover_command();
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args([
            "connect",
            vm_name,
            "--no-tmux",
            "--yes",
            "--",
            &discover_command,
        ]);

        match run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("===SESSION:") || stdout.contains("===DONE===") {
                    return self.parse_discovery_output(vm_name, &stdout);
                }
                if !output.status.success() {
                    return Vec::new();
                }
                self.parse_discovery_output(vm_name, &stdout)
            }
            Err(_) => Vec::new(),
        }
    }

    fn adopt_sessions(
        &self,
        vm_name: &str,
        queue: &mut TaskQueue,
        sessions: Option<&[String]>,
    ) -> Result<Vec<AdoptedSession>> {
        let mut discovered = self.discover_sessions(vm_name);
        if let Some(session_names) = sessions {
            discovered.retain(|session| {
                session_names
                    .iter()
                    .any(|name| name == &session.session_name)
            });
        }

        let mut adopted = Vec::new();
        for mut session in discovered {
            let prompt = if session.inferred_task.is_empty() {
                format!("Adopted session: {}", session.session_name)
            } else {
                session.inferred_task.clone()
            };
            let task = queue.add_task(
                &prompt,
                &session.inferred_repo,
                TaskPriority::Medium,
                if session.agent_type.is_empty() {
                    "claude"
                } else {
                    &session.agent_type
                },
                "auto",
                DEFAULT_MAX_TURNS,
            )?;
            if let Some(saved_task) = queue
                .tasks
                .iter_mut()
                .find(|candidate| candidate.id == task.id)
            {
                saved_task.assigned_vm = Some(vm_name.to_string());
                saved_task.assigned_session = Some(session.session_name.clone());
                saved_task.assigned_at = Some(now_isoformat());
                saved_task.started_at = Some(now_isoformat());
                saved_task.status = TaskStatus::Running;
            }
            queue.save()?;

            session.task_id = Some(task.id);
            session.adopted_at = Some(now_isoformat());
            adopted.push(session);
        }

        Ok(adopted)
    }

    fn build_discover_command(&self) -> String {
        [
            r##"for session in $(tmux list-sessions -F "#{session_name}" 2>/dev/null); do "##,
            r#"echo "===SESSION:$session==="; "#,
            r##"CWD=$(tmux display-message -t "$session" -p "#{pane_current_path}" 2>/dev/null); "##,
            r#"echo "CWD:$CWD"; "#,
            r##"CMD=$(tmux display-message -t "$session" -p "#{pane_current_command}" 2>/dev/null); "##,
            r#"echo "CMD:$CMD"; "#,
            r#"if [ -n "$CWD" ] && [ -d "$CWD/.git" ]; then "#,
            r#"BRANCH=$(cd "$CWD" && git branch --show-current 2>/dev/null); "#,
            r#"REMOTE=$(cd "$CWD" && git remote get-url origin 2>/dev/null); "#,
            r#"echo "BRANCH:$BRANCH"; "#,
            r#"echo "REPO:$REMOTE"; "#,
            r#"fi; "#,
            r#"echo "PANE_START"; "#,
            r#"tmux capture-pane -t "$session" -p -S -5 2>/dev/null | tail -5; "#,
            r#"echo "PANE_END"; "#,
            r#"done; "#,
            r#"echo "===DONE===""#,
        ]
        .concat()
    }

    fn parse_discovery_output(&self, vm_name: &str, output: &str) -> Vec<AdoptedSession> {
        let mut sessions = Vec::new();
        let mut current: Option<AdoptedSession> = None;

        for raw_line in output.lines() {
            let line = raw_line.trim();

            if line.starts_with("===SESSION:") && line.ends_with("===") {
                if let Some(session) = current.take() {
                    sessions.push(session);
                }
                let session_name = &line["===SESSION:".len()..line.len() - "===".len()];
                if session_name.is_empty() || session_name.starts_with('(') {
                    continue;
                }
                if validate_session_name(session_name).is_err() {
                    continue;
                }
                current = Some(AdoptedSession {
                    vm_name: vm_name.to_string(),
                    session_name: session_name.to_string(),
                    ..Default::default()
                });
                continue;
            }

            let Some(session) = current.as_mut() else {
                continue;
            };

            if let Some(value) = line.strip_prefix("CWD:") {
                session.working_directory = value.to_string();
            } else if let Some(value) = line.strip_prefix("CMD:") {
                let command = value.to_ascii_lowercase();
                if command.contains("claude") || command.contains("node") {
                    session.agent_type = "claude".to_string();
                } else if command.contains("amplifier") {
                    session.agent_type = "amplifier".to_string();
                } else if command.contains("copilot") {
                    session.agent_type = "copilot".to_string();
                }
            } else if let Some(value) = line.strip_prefix("BRANCH:") {
                session.inferred_branch = value.to_string();
            } else if let Some(value) = line.strip_prefix("REPO:") {
                session.inferred_repo = value.to_string();
            } else if let Some(value) = line.strip_prefix("PR:") {
                session.inferred_pr = value.to_string();
            } else if let Some(value) = line.strip_prefix("LAST_MSG:")
                && session.inferred_task.is_empty()
            {
                session.inferred_task = value.to_string();
            }
        }

        if let Some(session) = current {
            sessions.push(session);
        }

        sessions
    }
}

#[derive(Debug, Clone)]
struct FleetObserver {
    azlin_path: PathBuf,
    capture_lines: usize,
    previous_captures: BTreeMap<String, String>,
    last_change_time: BTreeMap<String, std::time::Instant>,
    stuck_threshold_seconds: f64,
}

impl FleetObserver {
    fn new(azlin_path: PathBuf) -> Self {
        Self {
            azlin_path,
            capture_lines: DEFAULT_CAPTURE_LINES,
            previous_captures: BTreeMap::new(),
            last_change_time: BTreeMap::new(),
            stuck_threshold_seconds: DEFAULT_STUCK_THRESHOLD_SECONDS,
        }
    }

    fn observe_session(&mut self, vm_name: &str, session_name: &str) -> Result<ObservationResult> {
        let pane_content = self.capture_pane(vm_name, session_name);
        let Some(pane_content) = pane_content else {
            return Ok(ObservationResult {
                session_name: session_name.to_string(),
                status: AgentStatus::Unknown,
                last_output_lines: Vec::new(),
                confidence: 0.0,
                matched_pattern: String::new(),
            });
        };

        let lines = pane_content
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let (status, confidence, pattern) = self.classify_output(&lines, vm_name, session_name);
        Ok(ObservationResult {
            session_name: session_name.to_string(),
            status,
            last_output_lines: lines,
            confidence,
            matched_pattern: pattern,
        })
    }

    fn observe_all(&self, sessions: &[TmuxSessionInfo]) -> Result<Vec<ObservationResult>> {
        let mut observer = self.clone();
        sessions
            .iter()
            .map(|session| observer.observe_session(&session.vm_name, &session.session_name))
            .collect()
    }

    fn capture_pane(&self, vm_name: &str, session_name: &str) -> Option<String> {
        if validate_vm_name(vm_name).is_err() || session_name.is_empty() {
            return None;
        }

        let session_name = shell_single_quote(session_name);
        let command = format!(
            "tmux capture-pane -t {session_name} -p -S -{} 2>/dev/null",
            self.capture_lines
        );
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", &command]);
        match run_output_with_timeout(cmd, TMUX_LIST_TIMEOUT) {
            Ok(output) if output.status.success() => {
                Some(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            Ok(_) | Err(_) => None,
        }
    }

    fn classify_output(
        &mut self,
        lines: &[String],
        vm_name: &str,
        session_name: &str,
    ) -> (AgentStatus, f64, String) {
        if lines.is_empty() {
            return (AgentStatus::Unknown, 0.0, String::new());
        }

        let combined = lines.join("\n");
        let key = format!("{vm_name}:{session_name}");
        let inferred = infer_agent_status(&combined);

        match inferred {
            AgentStatus::Completed => {
                return (
                    AgentStatus::Completed,
                    CONFIDENCE_COMPLETION,
                    "completion_detected".to_string(),
                );
            }
            AgentStatus::Error => {
                return (
                    AgentStatus::Error,
                    CONFIDENCE_ERROR,
                    "error_detected".to_string(),
                );
            }
            AgentStatus::WaitingInput => {
                return (
                    AgentStatus::WaitingInput,
                    CONFIDENCE_RUNNING,
                    "waiting_input_detected".to_string(),
                );
            }
            AgentStatus::Thinking => {
                self.last_change_time
                    .insert(key.clone(), std::time::Instant::now());
                self.previous_captures.insert(key, combined.clone());
                return (
                    AgentStatus::Thinking,
                    CONFIDENCE_THINKING,
                    "thinking_detected".to_string(),
                );
            }
            AgentStatus::Idle => {
                self.previous_captures.insert(key, combined);
                return (
                    AgentStatus::Idle,
                    CONFIDENCE_IDLE,
                    "idle_detected".to_string(),
                );
            }
            AgentStatus::Shell => {
                self.previous_captures.insert(key, combined);
                return (
                    AgentStatus::Shell,
                    CONFIDENCE_ERROR,
                    "shell_prompt".to_string(),
                );
            }
            AgentStatus::Unknown
            | AgentStatus::NoSession
            | AgentStatus::Unreachable
            | AgentStatus::Running
            | AgentStatus::Stuck => {}
        }

        if let Some(pattern) = first_matching_pattern(COMPLETION_PATTERNS, &combined, false) {
            return (AgentStatus::Completed, CONFIDENCE_COMPLETION, pattern);
        }
        if let Some(pattern) = first_matching_pattern(ERROR_PATTERNS, &combined, false) {
            return (AgentStatus::Error, CONFIDENCE_ERROR, pattern);
        }
        if let Some(pattern) = first_matching_pattern(RUNNING_PATTERNS, &combined, false) {
            self.last_change_time
                .insert(key.clone(), std::time::Instant::now());
            self.previous_captures.insert(key, combined);
            return (AgentStatus::Running, CONFIDENCE_RUNNING, pattern);
        }
        if let Some(pattern) = first_matching_pattern(WAITING_PATTERNS, &combined, true) {
            return (AgentStatus::WaitingInput, CONFIDENCE_RUNNING, pattern);
        }

        let now = std::time::Instant::now();
        if let Some(previous) = self.previous_captures.get(&key) {
            if previous == &combined {
                let last_change = self.last_change_time.get(&key).copied().unwrap_or(now);
                if now.duration_since(last_change).as_secs_f64() > self.stuck_threshold_seconds {
                    self.previous_captures.insert(key, combined);
                    return (
                        AgentStatus::Stuck,
                        CONFIDENCE_RUNNING,
                        "no_output_change".to_string(),
                    );
                }
            } else {
                self.last_change_time.insert(key.clone(), now);
            }
        } else {
            self.last_change_time.insert(key.clone(), now);
        }
        self.previous_captures.insert(key, combined.clone());

        let last_line = lines.last().map(String::as_str).unwrap_or("");
        if let Some(pattern) = first_matching_pattern(IDLE_PATTERNS, last_line, false) {
            return (AgentStatus::Idle, CONFIDENCE_IDLE, pattern);
        }

        if combined.trim().len() > MIN_SUBSTANTIAL_OUTPUT_LEN {
            return (
                AgentStatus::Running,
                CONFIDENCE_DEFAULT_RUNNING,
                "has_output".to_string(),
            );
        }

        (AgentStatus::Unknown, CONFIDENCE_UNKNOWN, String::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VmInfo {
    name: String,
    session_name: String,
    os: String,
    status: String,
    ip: String,
    region: String,
    tmux_sessions: Vec<TmuxSessionInfo>,
}

impl VmInfo {
    fn is_running(&self) -> bool {
        self.status.to_ascii_lowercase().contains("run")
    }

    fn active_agents(&self) -> usize {
        self.tmux_sessions
            .iter()
            .filter(|session| {
                matches!(
                    session.agent_status,
                    AgentStatus::Thinking | AgentStatus::Running | AgentStatus::WaitingInput
                )
            })
            .count()
    }
}

#[derive(Debug, Clone)]
struct FleetState {
    vms: Vec<VmInfo>,
    timestamp: Option<DateTime<Local>>,
    azlin_path: PathBuf,
    exclude_vms: Vec<String>,
}

impl FleetState {
    fn new(azlin_path: PathBuf) -> Self {
        Self {
            vms: Vec::new(),
            timestamp: None,
            azlin_path,
            exclude_vms: Vec::new(),
        }
    }

    fn exclude_vms(&mut self, vm_names: &[&str]) {
        self.exclude_vms
            .extend(vm_names.iter().map(|name| (*name).to_string()));
    }

    fn refresh(&mut self) {
        self.vms = self.poll_vms();
        self.timestamp = Some(Local::now());
        let azlin_path = self.azlin_path.clone();
        let excluded = self.exclude_vms.clone();

        for vm in &mut self.vms {
            if vm.is_running() && !excluded.iter().any(|name| name == &vm.name) {
                vm.tmux_sessions = Self::poll_tmux_sessions_with_path(&azlin_path, &vm.name);
            }
        }
    }

    fn summary(&self) -> String {
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

    fn managed_vms(&self) -> Vec<&VmInfo> {
        self.vms
            .iter()
            .filter(|vm| !self.exclude_vms.iter().any(|name| name == &vm.name))
            .collect()
    }

    fn all_vms(&self) -> Vec<&VmInfo> {
        self.vms.iter().collect()
    }

    /// Returns `true` if `vm_name` is not in the exclude list.
    ///
    /// Used for managed/unmanaged labeling in the AllSessions subview.
    #[allow(dead_code)]
    fn is_managed_vm(&self, vm_name: &str) -> bool {
        !self.exclude_vms.iter().any(|name| name == vm_name)
    }

    fn idle_vms(&self) -> Vec<&VmInfo> {
        self.managed_vms()
            .into_iter()
            .filter(|vm| vm.is_running() && vm.active_agents() == 0)
            .collect()
    }

    fn get_vm(&self, vm_name: &str) -> Option<&VmInfo> {
        self.vms.iter().find(|vm| vm.name == vm_name)
    }

    fn poll_vms(&self) -> Vec<VmInfo> {
        let mut json_cmd = Command::new(&self.azlin_path);
        json_cmd.args(["list", "--json"]);
        match run_output_with_timeout(json_cmd, AZLIN_LIST_TIMEOUT) {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    let parsed = Self::parse_vm_json(&stdout);
                    if !parsed.is_empty() || stdout.trim() == "[]" {
                        return parsed;
                    }
                }
            }
            Ok(_) | Err(_) => {}
        }

        let mut text_cmd = Command::new(&self.azlin_path);
        text_cmd.arg("list");
        match run_output_with_timeout(text_cmd, AZLIN_LIST_TIMEOUT) {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Self::parse_vm_text(&stdout)
            }
            Ok(_) | Err(_) => Vec::new(),
        }
    }

    fn parse_vm_json(json_str: &str) -> Vec<VmInfo> {
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

    fn parse_vm_text(text: &str) -> Vec<VmInfo> {
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
    fn poll_tmux_sessions(&self, vm_name: &str) -> Vec<TmuxSessionInfo> {
        Self::poll_tmux_sessions_with_path(&self.azlin_path, vm_name)
    }

    fn poll_tmux_sessions_with_path(azlin_path: &Path, vm_name: &str) -> Vec<TmuxSessionInfo> {
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
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn run_output_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn().context("failed to spawn subprocess")?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel::<std::io::Result<Output>>();

    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result.context("failed to wait for subprocess output"),
        Err(_elapsed) => {
            #[cfg(unix)]
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
            bail!(
                "subprocess timed out after {} seconds (pid {})",
                timeout.as_secs(),
                pid
            )
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::command_error;
    use crate::test_support::home_env_lock;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn write_executable(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn parses_native_setup_command() {
        match parse_native_fleet_command(&[String::from("setup")]) {
            Some(NativeFleetCommand::Setup) => {}
            other => panic!("expected setup command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_status_command() {
        match parse_native_fleet_command(&[String::from("status")]) {
            Some(NativeFleetCommand::Status) => {}
            other => panic!("expected status command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_snapshot_command() {
        match parse_native_fleet_command(&[String::from("snapshot")]) {
            Some(NativeFleetCommand::Snapshot) => {}
            other => panic!("expected snapshot command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_tui_command() {
        match parse_native_fleet_command(&[
            String::from("tui"),
            String::from("--interval"),
            String::from("10"),
            String::from("--capture-lines"),
            String::from("80"),
        ]) {
            Some(NativeFleetCommand::Tui {
                interval,
                capture_lines,
            }) => {
                assert_eq!(interval, 10);
                assert_eq!(capture_lines, 80);
            }
            other => panic!("expected tui command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_start_command() {
        match parse_native_fleet_command(&[
            String::from("start"),
            String::from("--max-cycles"),
            String::from("2"),
            String::from("--interval"),
            String::from("15"),
            String::from("--adopt"),
            String::from("--stuck-threshold"),
            String::from("45"),
            String::from("--max-agents-per-vm"),
            String::from("4"),
            String::from("--capture-lines"),
            String::from("80"),
        ]) {
            Some(NativeFleetCommand::Start {
                max_cycles,
                interval,
                adopt,
                stuck_threshold,
                max_agents_per_vm,
                capture_lines,
            }) => {
                assert_eq!(max_cycles, 2);
                assert_eq!(interval, 15);
                assert!(adopt);
                assert_eq!(stuck_threshold, 45.0);
                assert_eq!(max_agents_per_vm, 4);
                assert_eq!(capture_lines, 80);
            }
            other => panic!("expected start command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_run_once_command() {
        match parse_native_fleet_command(&[String::from("run-once")]) {
            Some(NativeFleetCommand::RunOnce) => {}
            other => panic!("expected run-once command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_dry_run_command() {
        match parse_native_fleet_command(&[
            String::from("dry-run"),
            String::from("--vm"),
            String::from("vm-1"),
            String::from("--priorities"),
            String::from("Quality first"),
            String::from("--backend"),
            String::from("auto"),
        ]) {
            Some(NativeFleetCommand::DryRun {
                vm,
                priorities,
                backend,
            }) => {
                assert_eq!(vm, vec!["vm-1".to_string()]);
                assert_eq!(priorities, "Quality first");
                assert_eq!(backend, "auto");
            }
            other => panic!("expected dry-run command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_scout_command() {
        match parse_native_fleet_command(&[
            String::from("scout"),
            String::from("--vm"),
            String::from("vm-1"),
            String::from("--session"),
            String::from("vm-1:work-1"),
            String::from("--skip-adopt"),
            String::from("--incremental"),
            String::from("--save"),
            String::from("/tmp/scout.json"),
        ]) {
            Some(NativeFleetCommand::Scout {
                vm,
                session_target,
                skip_adopt,
                incremental,
                save_path,
            }) => {
                assert_eq!(vm.as_deref(), Some("vm-1"));
                assert_eq!(session_target.as_deref(), Some("vm-1:work-1"));
                assert!(skip_adopt);
                assert!(incremental);
                assert_eq!(save_path.as_deref(), Some(Path::new("/tmp/scout.json")));
            }
            other => panic!("expected scout command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_advance_command() {
        match parse_native_fleet_command(&[
            String::from("advance"),
            String::from("--vm"),
            String::from("vm-1"),
            String::from("--session"),
            String::from("vm-1:work-1"),
            String::from("--force"),
            String::from("--save"),
            String::from("/tmp/advance.json"),
        ]) {
            Some(NativeFleetCommand::Advance {
                vm,
                session_target,
                force,
                save_path,
            }) => {
                assert_eq!(vm.as_deref(), Some("vm-1"));
                assert_eq!(session_target.as_deref(), Some("vm-1:work-1"));
                assert!(force);
                assert_eq!(save_path.as_deref(), Some(Path::new("/tmp/advance.json")));
            }
            other => panic!("expected advance command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_auth_command() {
        match parse_native_fleet_command(&[
            String::from("auth"),
            String::from("vm-1"),
            String::from("--services"),
            String::from("github"),
            String::from("--services"),
            String::from("azure"),
        ]) {
            Some(NativeFleetCommand::Auth { vm_name, services }) => {
                assert_eq!(vm_name, "vm-1");
                assert_eq!(services, vec!["github".to_string(), "azure".to_string()]);
            }
            other => panic!("expected auth command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_adopt_command() {
        match parse_native_fleet_command(&[
            String::from("adopt"),
            String::from("vm-1"),
            String::from("--sessions"),
            String::from("work-1"),
            String::from("--sessions"),
            String::from("work-2"),
        ]) {
            Some(NativeFleetCommand::Adopt { vm_name, sessions }) => {
                assert_eq!(vm_name, "vm-1");
                assert_eq!(sessions, vec!["work-1".to_string(), "work-2".to_string()]);
            }
            other => panic!("expected adopt command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_observe_command() {
        match parse_native_fleet_command(&[String::from("observe"), String::from("vm-1")]) {
            Some(NativeFleetCommand::Observe { vm_name }) => assert_eq!(vm_name, "vm-1"),
            other => panic!("expected observe command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_report_command() {
        match parse_native_fleet_command(&[String::from("report")]) {
            Some(NativeFleetCommand::Report) => {}
            other => panic!("expected report command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_queue_command() {
        match parse_native_fleet_command(&[String::from("queue")]) {
            Some(NativeFleetCommand::Queue) => {}
            other => panic!("expected queue command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_add_task_command_with_defaults() {
        match parse_native_fleet_command(&[
            String::from("add-task"),
            String::from("Fix the login bug"),
        ]) {
            Some(NativeFleetCommand::AddTask {
                prompt,
                repo,
                priority,
                agent,
                mode,
                max_turns,
                protected,
            }) => {
                assert_eq!(prompt, "Fix the login bug");
                assert_eq!(repo, "");
                assert!(matches!(priority, NativeTaskPriorityArg::Medium));
                assert!(matches!(agent, NativeAgentArg::Claude));
                assert!(matches!(mode, NativeAgentModeArg::Auto));
                assert_eq!(max_turns, DEFAULT_MAX_TURNS);
                assert!(!protected);
            }
            other => panic!("expected add-task command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_graph_command() {
        match parse_native_fleet_command(&[String::from("graph")]) {
            Some(NativeFleetCommand::Graph) => {}
            other => panic!("expected graph command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_copilot_status_command() {
        match parse_native_fleet_command(&[String::from("copilot-status")]) {
            Some(NativeFleetCommand::CopilotStatus) => {}
            other => panic!("expected copilot-status command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_copilot_log_command() {
        match parse_native_fleet_command(&[
            String::from("copilot-log"),
            String::from("--tail"),
            String::from("3"),
        ]) {
            Some(NativeFleetCommand::CopilotLog { tail }) => assert_eq!(tail, 3),
            other => panic!("expected copilot-log command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_dashboard_command() {
        match parse_native_fleet_command(&[String::from("dashboard")]) {
            Some(NativeFleetCommand::Dashboard) => {}
            other => panic!("expected dashboard command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_watch_command() {
        match parse_native_fleet_command(&[
            String::from("watch"),
            String::from("test-vm"),
            String::from("session-1"),
        ]) {
            Some(NativeFleetCommand::Watch {
                vm_name,
                session_name,
                lines,
            }) => {
                assert_eq!(vm_name, "test-vm");
                assert_eq!(session_name, "session-1");
                assert_eq!(lines, 30);
            }
            other => panic!("expected watch command, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_project_add_command() {
        match parse_native_fleet_command(&[
            String::from("project"),
            String::from("add"),
            String::from("https://github.com/org/repo"),
            String::from("--identity"),
            String::from("bot-account"),
            String::from("--priority"),
            String::from("high"),
            String::from("--name"),
            String::from("custom-name"),
        ]) {
            Some(NativeFleetCommand::Project {
                command:
                    NativeFleetProjectCommand::Add {
                        repo_url,
                        identity,
                        priority,
                        name,
                    },
            }) => {
                assert_eq!(repo_url, "https://github.com/org/repo");
                assert_eq!(identity, "bot-account");
                assert!(matches!(priority, NativeProjectPriorityArg::High));
                assert_eq!(name, "custom-name");
            }
            other => panic!("expected project add command, got {other:?}"),
        }
    }

    #[test]
    fn empty_and_help_fleet_commands_bypass_subcommand_parsing() {
        assert!(parse_native_fleet_command(&[]).is_none());
        assert!(
            parse_native_fleet_command(&[String::from("status"), String::from("--help")]).is_none()
        );
    }

    #[test]
    fn azlin_path_prefers_environment_override() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("custom-azlin");
        write_executable(&azlin, "#!/bin/sh\nexit 0\n");

        let previous = env::var_os("AZLIN_PATH");
        unsafe { env::set_var("AZLIN_PATH", &azlin) };

        let found = get_azlin_path().unwrap();

        match previous {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }

        assert_eq!(found, azlin);
    }

    #[test]
    fn run_setup_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(&azlin, "#!/bin/sh\necho 'azlin 1.2.3'\n");

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("AZLIN_PATH", &azlin);
            env::set_var("PATH", dir.path());
            env::set_var("HOME", home.path());
        }

        let result = run_setup();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn run_setup_returns_exit_error_when_azlin_missing() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let empty_path = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::remove_var("AZLIN_PATH");
            env::set_var("PATH", empty_path.path());
            env::set_var("HOME", home.path());
        }

        let result = run_setup();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        let err = result.expect_err("missing azlin should fail");
        assert_eq!(command_error::exit_code(&err), Some(1));
    }

    #[test]
    fn parse_vm_json_supports_list_and_location_fallback() {
        let vms = FleetState::parse_vm_json(
            r#"[{"name":"vm-1","status":"Running","location":"eastus"}]"#,
        );
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].name, "vm-1");
        assert_eq!(vms[0].region, "eastus");
    }

    #[test]
    fn parse_vm_json_supports_dict_wrapped_vms() {
        let vms = FleetState::parse_vm_json(
            r#"{"vms":[{"name":"vm-2","status":"Stopped","region":"westus2"}]}"#,
        );
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].name, "vm-2");
        assert_eq!(vms[0].status, "Stopped");
    }

    #[test]
    fn parse_vm_text_extracts_rows() {
        let text = concat!(
            "│ Session     │ Tmux │ OS     │ Status  │ IP       │ Region  │\n",
            "┣━━━━━━━━━━━━━╋━━━━━━╋━━━━━━━━╋━━━━━━━━━╋━━━━━━━━━━╋━━━━━━━━━┫\n",
            "│ fleet-vm-1  │ yes  │ Ubuntu │ Running │ 10.0.0.5 │ westus2 │\n",
            "│ fleet-vm-2  │ no   │ Ubuntu │ Stopped │ 10.0.0.6 │ eastus  │\n"
        );
        let vms = FleetState::parse_vm_text(text);
        assert_eq!(vms.len(), 2);
        assert_eq!(vms[0].name, "fleet-vm-1");
        assert_eq!(vms[0].status, "Running");
        assert_eq!(vms[1].region, "eastus");
    }

    #[test]
    fn poll_tmux_sessions_parses_multiple_sessions() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            "#!/bin/sh\nif [ \"$1\" = connect ]; then\n  printf \"amplihack-ultra|||1|||1\\nbart|||2|||0\\n\";\nelse\n  exit 1\nfi\n",
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("AZLIN_PATH", &azlin);
            env::set_var("PATH", dir.path());
            env::set_var("HOME", home.path());
        }

        let state = FleetState::new(azlin.clone());
        let sessions = state.poll_tmux_sessions("test-vm");

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_name, "amplihack-ultra");
        assert_eq!(sessions[0].windows, 1);
        assert!(sessions[0].attached);
        assert_eq!(sessions[1].windows, 2);
        assert!(!sessions[1].attached);
    }

    #[test]
    fn summary_uses_expected_icons() {
        let mut state = FleetState::new(PathBuf::from("/tmp/azlin"));
        state.vms = vec![
            VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: String::new(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus".to_string(),
                tmux_sessions: vec![
                    TmuxSessionInfo {
                        session_name: "s1".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::Completed,
                        last_output: String::new(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    },
                    TmuxSessionInfo {
                        session_name: "s2".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::Stuck,
                        last_output: String::new(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    },
                ],
            },
            VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: String::new(),
                status: "Stopped".to_string(),
                ip: String::new(),
                region: "eastus".to_string(),
                tmux_sessions: Vec::new(),
            },
        ];

        let summary = state.summary();

        assert!(summary.contains("[+] vm-1 (westus) - Running"));
        assert!(summary.contains("[=] s1 (completed)"));
        assert!(summary.contains("[!] s2 (stuck)"));
        assert!(summary.contains("[-] vm-2 (eastus) - Stopped"));
    }

    #[test]
    fn task_queue_summary_matches_python_shape() {
        let queue = TaskQueue {
            tasks: vec![
                FleetTask {
                    id: "abc123".to_string(),
                    prompt: "High priority task".to_string(),
                    repo_url: String::new(),
                    branch: String::new(),
                    priority: TaskPriority::High,
                    status: TaskStatus::Queued,
                    agent_command: "claude".to_string(),
                    agent_mode: "auto".to_string(),
                    max_turns: DEFAULT_MAX_TURNS,
                    protected: false,
                    assigned_vm: None,
                    assigned_session: None,
                    assigned_at: None,
                    created_at: now_isoformat(),
                    started_at: None,
                    completed_at: None,
                    result: None,
                    pr_url: None,
                    error: None,
                },
                FleetTask {
                    id: "def456".to_string(),
                    prompt: "Assigned task".to_string(),
                    repo_url: String::new(),
                    branch: String::new(),
                    priority: TaskPriority::Low,
                    status: TaskStatus::Assigned,
                    agent_command: "claude".to_string(),
                    agent_mode: "auto".to_string(),
                    max_turns: DEFAULT_MAX_TURNS,
                    protected: false,
                    assigned_vm: Some("vm-1".to_string()),
                    assigned_session: None,
                    assigned_at: None,
                    created_at: now_isoformat(),
                    started_at: None,
                    completed_at: None,
                    result: None,
                    pr_url: None,
                    error: None,
                },
            ],
            persist_path: None,
            load_failed: false,
        };

        let summary = queue.summary();
        assert!(summary.contains("Task Queue (2 tasks)"));
        assert!(summary.contains("  QUEUED (1):"));
        assert!(summary.contains("    [H] abc123: High priority task"));
        assert!(summary.contains("  ASSIGNED (1):"));
        assert!(summary.contains("    [L] def456: Assigned task -> vm-1"));
    }

    #[test]
    fn task_queue_persists_and_loads_python_compatible_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task_queue.json");

        let mut queue = TaskQueue::load(Some(path.clone())).unwrap();
        let task = queue
            .add_task(
                "Persistent task",
                "https://github.com/org/repo",
                TaskPriority::High,
                "amplifier",
                "ultrathink",
                50,
            )
            .unwrap();

        let loaded = TaskQueue::load(Some(path.clone())).unwrap();
        assert_eq!(loaded.tasks.len(), 1);
        assert_eq!(loaded.tasks[0].id, task.id);
        assert_eq!(loaded.tasks[0].repo_url, "https://github.com/org/repo");
        assert_eq!(loaded.tasks[0].agent_command, "amplifier");
        assert_eq!(loaded.tasks[0].agent_mode, "ultrathink");
        assert_eq!(loaded.tasks[0].max_turns, 50);
    }

    #[test]
    fn run_add_task_creates_default_queue_file() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("HOME", home.path()) };

        let result = run_add_task(
            "Refactor auth module",
            "https://github.com/org/repo",
            NativeTaskPriorityArg::High,
            NativeAgentArg::Amplifier,
            NativeAgentModeArg::Ultrathink,
            50,
            false,
        );

        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());

        let queue_path = home.path().join(".amplihack/fleet/task_queue.json");
        let loaded = TaskQueue::load(Some(queue_path)).unwrap();
        assert_eq!(loaded.tasks.len(), 1);
        assert_eq!(loaded.tasks[0].prompt, "Refactor auth module");
        assert_eq!(loaded.tasks[0].repo_url, "https://github.com/org/repo");
        assert_eq!(loaded.tasks[0].priority, TaskPriority::High);
    }

    #[test]
    fn fleet_graph_summary_matches_python_shape() {
        let graph = FleetGraphSummary {
            node_types: vec![
                "project".to_string(),
                "task".to_string(),
                "task".to_string(),
            ],
            edge_types: vec!["contains".to_string(), "conflicts".to_string()],
        };

        let summary = graph.summary();
        assert!(summary.contains("Fleet Graph: 3 nodes, 2 edges"));
        assert!(summary.contains("  Nodes: project=1, task=2"));
        assert!(summary.contains("  Edges: conflicts=1, contains=1"));
        assert!(summary.contains("  !! 1 conflicts detected"));
    }

    #[test]
    fn fleet_graph_loads_python_json_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.json");
        fs::write(
            &path,
            r#"{
  "nodes": {
    "proj-1": {"type": "project", "label": "proj-1", "metadata": {}},
    "task-1": {"type": "task", "label": "task-1", "metadata": {}}
  },
  "edges": [
    {"source": "proj-1", "target": "task-1", "type": "contains", "metadata": {}}
  ]
}"#,
        )
        .unwrap();

        let graph = FleetGraphSummary::load(Some(path)).unwrap();
        assert_eq!(graph.node_types.len(), 2);
        assert_eq!(graph.edge_types, vec!["contains".to_string()]);
    }

    #[test]
    fn fleet_dashboard_summary_matches_python_shape() {
        let dashboard = FleetDashboardSummary {
            projects: vec![ProjectInfo {
                repo_url: "https://github.com/org/repo".to_string(),
                name: "repo".to_string(),
                github_identity: "user1".to_string(),
                priority: "medium".to_string(),
                notes: String::new(),
                vms: vec!["vm-01".to_string()],
                tasks_total: 4,
                tasks_completed: 2,
                tasks_failed: 1,
                tasks_in_progress: 1,
                prs_created: vec!["pr-url".to_string()],
                estimated_cost_usd: 5.25,
                started_at: Some(now_isoformat()),
                last_activity: Some(now_isoformat()),
            }],
            persist_path: None,
            load_failed: false,
        };

        let summary = dashboard.summary();
        assert!(summary.contains("FLEET DASHBOARD"));
        assert!(summary.contains("Projects: 1"));
        assert!(summary.contains("Tasks: 2/4 completed"));
        assert!(summary.contains("PRs created: 1"));
        assert!(summary.contains("Estimated cost: $5.25"));
        assert!(summary.contains("[repo] (user1)"));
        assert!(summary.contains("!! 1 failed tasks"));
    }

    #[test]
    fn fleet_dashboard_updates_from_queue() {
        let mut dashboard = FleetDashboardSummary {
            projects: Vec::new(),
            persist_path: None,
            load_failed: false,
        };
        let mut queue = TaskQueue {
            tasks: Vec::new(),
            persist_path: None,
            load_failed: false,
        };
        let mut completed = FleetTask::new(
            "Fix bug",
            "https://github.com/org/repo",
            TaskPriority::High,
            "claude",
            "auto",
            DEFAULT_MAX_TURNS,
        );
        completed.status = TaskStatus::Completed;
        completed.pr_url = Some("https://github.com/org/repo/pull/1".to_string());
        let mut assigned = FleetTask::new(
            "Add auth",
            "https://github.com/org/repo",
            TaskPriority::Medium,
            "claude",
            "auto",
            DEFAULT_MAX_TURNS,
        );
        assigned.status = TaskStatus::Assigned;
        assigned.assigned_vm = Some("vm-01".to_string());
        queue.tasks = vec![completed, assigned];

        dashboard.update_from_queue(&queue).unwrap();

        assert_eq!(dashboard.projects.len(), 1);
        let project = &dashboard.projects[0];
        assert_eq!(project.tasks_total, 2);
        assert_eq!(project.tasks_completed, 1);
        assert_eq!(project.tasks_in_progress, 1);
        assert_eq!(
            project.prs_created,
            vec!["https://github.com/org/repo/pull/1"]
        );
        assert_eq!(project.vms, vec!["vm-01".to_string()]);
    }

    #[test]
    fn run_project_add_persists_dashboard_and_projects_toml() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("HOME", home.path()) };

        let result = run_project_add(
            "https://github.com/org/my-repo",
            "bot-account",
            "high",
            "custom-name",
        );

        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());

        let dashboard =
            FleetDashboardSummary::load(Some(home.path().join(".amplihack/fleet/dashboard.json")))
                .unwrap();
        assert_eq!(dashboard.projects.len(), 1);
        assert_eq!(dashboard.projects[0].name, "custom-name");
        assert_eq!(dashboard.projects[0].github_identity, "bot-account");

        let projects =
            load_projects_registry(&home.path().join(".amplihack/fleet/projects.toml")).unwrap();
        assert!(projects.contains_key("custom-name"));
        assert_eq!(
            projects["custom-name"].repo_url,
            "https://github.com/org/my-repo"
        );
    }

    #[test]
    fn run_project_remove_only_updates_dashboard() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("HOME", home.path()) };

        run_project_add("https://github.com/org/my-repo", "", "medium", "my-repo").unwrap();
        let result = run_project_remove("my-repo");

        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());

        let dashboard =
            FleetDashboardSummary::load(Some(home.path().join(".amplihack/fleet/dashboard.json")))
                .unwrap();
        assert!(dashboard.projects.is_empty());

        let projects =
            load_projects_registry(&home.path().join(".amplihack/fleet/projects.toml")).unwrap();
        assert!(projects.contains_key("my-repo"));
    }

    #[test]
    fn render_project_list_matches_python_shape() {
        let dashboard = FleetDashboardSummary {
            projects: vec![ProjectInfo {
                name: "repo".to_string(),
                repo_url: "https://github.com/org/repo".to_string(),
                github_identity: "user1".to_string(),
                priority: "high".to_string(),
                tasks_total: 4,
                tasks_completed: 2,
                tasks_failed: 0,
                tasks_in_progress: 2,
                vms: vec!["vm-01".to_string(), "vm-02".to_string()],
                notes: "tracking auth work".to_string(),
                prs_created: vec!["https://github.com/org/repo/pull/1".to_string()],
                estimated_cost_usd: 0.0,
                started_at: None,
                last_activity: None,
            }],
            persist_path: None,
            load_failed: false,
        };

        let rendered = render_project_list(&dashboard);
        assert!(rendered.contains("Fleet Projects (1)"));
        assert!(rendered.contains("[!!!] repo"));
        assert!(rendered.contains("Repo: https://github.com/org/repo"));
        assert!(rendered.contains("Identity: user1"));
        assert!(rendered.contains("Priority: high"));
        assert!(rendered.contains("VMs: 2 | Tasks: 2/4 | PRs: 1"));
        assert!(rendered.contains("Notes: tracking auth work"));
    }

    #[test]
    fn render_copilot_status_matches_python_cases() {
        let temp = tempfile::tempdir().unwrap();
        let lock_dir = temp.path();

        assert_eq!(
            render_copilot_status(lock_dir).unwrap(),
            "Copilot: not active"
        );

        fs::write(lock_dir.join(".lock_active"), "locked").unwrap();
        assert_eq!(
            render_copilot_status(lock_dir).unwrap(),
            "Copilot: active (no goal)"
        );

        fs::write(lock_dir.join(".lock_goal"), "Fix authentication bug\n").unwrap();
        assert_eq!(
            render_copilot_status(lock_dir).unwrap(),
            "Copilot: active\nGoal: Fix authentication bug"
        );
    }

    #[test]
    fn read_copilot_log_matches_python_shape() {
        let temp = tempfile::tempdir().unwrap();
        let log_dir = temp.path();
        let report = read_copilot_log(log_dir, 0).unwrap();
        assert_eq!(report.rendered, "No decisions recorded.");
        assert_eq!(report.malformed_entries, 0);

        fs::write(log_dir.join("decisions.jsonl"), "").unwrap();
        let report = read_copilot_log(log_dir, 0).unwrap();
        assert_eq!(report.rendered, "No decisions recorded.");
        assert_eq!(report.malformed_entries, 0);

        fs::write(
            log_dir.join("decisions.jsonl"),
            r#"{"timestamp":"2026-03-03T10:00:00","action":"send_input","reasoning":"Agent is idle at prompt","confidence":0.85}
{"timestamp":"2026-03-03T10:05:00","action":"wait","reasoning":"Agent has a tool call in flight","confidence":0.95}"#,
        )
        .unwrap();
        let report = read_copilot_log(log_dir, 0).unwrap();
        assert!(
            report
                .rendered
                .contains("[2026-03-03T10:00:00] send_input (confidence=0.85)")
        );
        assert!(report.rendered.contains("Agent is idle at prompt"));
        assert!(
            report
                .rendered
                .contains("[2026-03-03T10:05:00] wait (confidence=0.95)")
        );
        assert_eq!(report.malformed_entries, 0);
    }

    #[test]
    fn read_copilot_log_applies_tail_and_tracks_malformed_entries() {
        let temp = tempfile::tempdir().unwrap();
        let log_dir = temp.path();
        fs::write(
            log_dir.join("decisions.jsonl"),
            r#"{"timestamp":"2026-03-03T10:00:00","action":"action_0","reasoning":"reason_0","confidence":0.8}
not-json
{"timestamp":"2026-03-03T10:08:00","action":"action_8","reasoning":"reason_8","confidence":0.8}
{"timestamp":"2026-03-03T10:09:00","action":"action_9","reasoning":"reason_9","confidence":0.8}"#,
        )
        .unwrap();

        let report = read_copilot_log(log_dir, 2).unwrap();
        assert!(!report.rendered.contains("action_0"));
        assert!(report.rendered.contains("action_8"));
        assert!(report.rendered.contains("action_9"));
        assert_eq!(report.malformed_entries, 1);
    }

    #[test]
    fn observer_classifies_running_and_completed_output() {
        let azlin = PathBuf::from("/bin/true");
        let mut observer = FleetObserver::new(azlin);

        let (status, confidence, pattern) = observer.classify_output(
            &[
                "Step 5: Implementing authentication module".to_string(),
                "Reading file auth.py".to_string(),
            ],
            "vm-1",
            "sess-1",
        );
        assert_eq!(status, AgentStatus::Running);
        assert_eq!(confidence, CONFIDENCE_RUNNING);
        assert_eq!(pattern, r"Step \d+");

        let (status, confidence, pattern) = observer.classify_output(
            &[
                "Step 22: Creating pull request".to_string(),
                "PR #42 created: https://github.com/org/repo/pull/42".to_string(),
            ],
            "vm-1",
            "sess-1",
        );
        assert_eq!(status, AgentStatus::Completed);
        assert_eq!(confidence, CONFIDENCE_COMPLETION);
        assert_eq!(pattern, "completion_detected");
    }

    #[test]
    fn observer_classifies_waiting_and_idle_output() {
        let azlin = PathBuf::from("/bin/true");
        let mut observer = FleetObserver::new(azlin);

        let (status, _, _) = observer.classify_output(
            &["Continue with this approach? [Y/n]".to_string()],
            "vm-1",
            "sess-1",
        );
        assert_eq!(status, AgentStatus::WaitingInput);

        let (status, _, _) = observer.classify_output(
            &["azureuser@fleet-exp-1:~/code$ ".to_string()],
            "vm-1",
            "sess-2",
        );
        assert_eq!(status, AgentStatus::Shell);
    }

    #[test]
    fn auth_propagator_rejects_unknown_service() {
        let auth = AuthPropagator::new(PathBuf::from("/bin/true"));
        let results = auth.propagate_all("vm-1", &[String::from("nonexistent")]);
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert_eq!(results[0].service, "nonexistent");
        assert_eq!(results[0].vm_name, "vm-1");
        assert!(
            results[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Unknown service")
        );
    }

    #[test]
    fn validate_chmod_mode_rejects_unsafe_values() {
        assert!(validate_chmod_mode("600").is_ok());
        assert!(validate_chmod_mode("0644").is_ok());
        assert!(validate_chmod_mode("abc").is_err());
        assert!(validate_chmod_mode("999").is_err());
    }

    #[test]
    fn parse_discovery_output_extracts_session_context() {
        let adopter = SessionAdopter::new(PathBuf::from("/bin/true"));
        let sessions = adopter.parse_discovery_output(
            "vm-01",
            "===SESSION:dev-1===\nCWD:/workspace/myrepo\nCMD:node /usr/local/bin/claude\nBRANCH:feat/login\nREPO:https://github.com/org/myrepo.git\nPR:https://github.com/org/myrepo/pull/42\nLAST_MSG:Implementing authentication\n===DONE===\n",
        );

        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.vm_name, "vm-01");
        assert_eq!(session.session_name, "dev-1");
        assert_eq!(session.working_directory, "/workspace/myrepo");
        assert_eq!(session.agent_type, "claude");
        assert_eq!(session.inferred_branch, "feat/login");
        assert_eq!(session.inferred_repo, "https://github.com/org/myrepo.git");
        assert_eq!(session.inferred_pr, "https://github.com/org/myrepo/pull/42");
        assert_eq!(session.inferred_task, "Implementing authentication");
    }

    #[test]
    fn adopt_sessions_creates_running_tasks() {
        let adopter = SessionAdopter::new(PathBuf::from("/bin/true"));
        let mut queue = TaskQueue {
            tasks: Vec::new(),
            persist_path: None,
            load_failed: false,
        };
        let output = "===SESSION:work-1===\nCMD:claude\nREPO:https://github.com/org/repo.git\nLAST_MSG:Working on feature X\n===DONE===\n";
        let sessions = adopter.parse_discovery_output("vm-01", output);

        let mut adopted = Vec::new();
        for mut session in sessions {
            let task = queue
                .add_task(
                    &session.inferred_task,
                    &session.inferred_repo,
                    TaskPriority::Medium,
                    "claude",
                    "auto",
                    DEFAULT_MAX_TURNS,
                )
                .unwrap();
            if let Some(saved_task) = queue
                .tasks
                .iter_mut()
                .find(|candidate| candidate.id == task.id)
            {
                saved_task.assigned_vm = Some("vm-01".to_string());
                saved_task.assigned_session = Some(session.session_name.clone());
                saved_task.assigned_at = Some(now_isoformat());
                saved_task.started_at = Some(now_isoformat());
                saved_task.status = TaskStatus::Running;
            }
            session.task_id = Some(task.id);
            adopted.push(session);
        }

        assert_eq!(adopted.len(), 1);
        assert!(adopted[0].task_id.is_some());
        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(queue.tasks[0].status, TaskStatus::Running);
        assert_eq!(queue.tasks[0].assigned_vm.as_deref(), Some("vm-01"));
        assert_eq!(queue.tasks[0].assigned_session.as_deref(), Some("work-1"));
    }

    #[test]
    fn render_snapshot_matches_python_shape_for_empty_fleet() {
        let state = FleetState {
            vms: Vec::new(),
            timestamp: None,
            azlin_path: PathBuf::from("/bin/true"),
            exclude_vms: Vec::new(),
        };
        let mut observer = FleetObserver::new(PathBuf::from("/bin/true"));
        let rendered = render_snapshot(&state, &mut observer).unwrap();
        assert_eq!(
            rendered,
            format!("Fleet Snapshot (0 managed VMs)\n{}", "=".repeat(60))
        );
    }

    #[test]
    fn render_report_matches_python_shape() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "ubuntu".to_string(),
                status: "Running".to_string(),
                ip: "10.0.0.1".to_string(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: String::new(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("/bin/true"),
            exclude_vms: Vec::new(),
        };
        let queue = TaskQueue {
            tasks: vec![FleetTask::new(
                "Working on feature X",
                "https://github.com/org/repo.git",
                TaskPriority::Medium,
                "claude",
                "auto",
                DEFAULT_MAX_TURNS,
            )],
            persist_path: None,
            load_failed: false,
        };

        let rendered = render_report(&state, &queue);
        assert!(rendered.contains("Fleet Admiral Report — Cycle 0"));
        assert!(rendered.contains("Fleet State"));
        assert!(rendered.contains("Task Queue (1 tasks)"));
        assert!(rendered.contains("Admiral log: 0 actions recorded"));
        assert!(rendered.contains("Stats: 0 actions, 0 successes, 0 failures"));
    }

    #[test]
    fn run_status_returns_exit_error_when_azlin_missing() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let empty_path = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::remove_var("AZLIN_PATH");
            env::set_var("PATH", empty_path.path());
            env::set_var("HOME", home.path());
        }

        let result = run_status();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        let err = result.expect_err("missing azlin should fail");
        assert_eq!(command_error::exit_code(&err), Some(1));
    }

    #[test]
    fn run_status_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            "#!/bin/sh\nif [ \"$1\" = list ] && [ \"$2\" = \"--json\" ]; then\n  echo '[{\"name\":\"vm-1\",\"status\":\"Running\",\"region\":\"westus2\"}]'\nelif [ \"$1\" = connect ]; then\n  printf \"work|||1|||1\\n\";\nelse\n  exit 1\nfi\n",
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("AZLIN_PATH", &azlin);
            env::set_var("PATH", dir.path());
            env::set_var("HOME", home.path());
        }

        let result = run_status();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn run_snapshot_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
    *"tmux list-sessions"*)
      printf '%s\n' "claude-1|||1|||0"
      exit 0
      ;;
    *"tmux capture-pane -t 'claude-1'"*)
      printf '%s\n' "Step 5: Implementing auth" "Reading file auth.py" "Running tests"
      exit 0
      ;;
  esac
fi
exit 1
"#,
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("AZLIN_PATH", &azlin) };
        unsafe { env::set_var("HOME", home.path()) };

        let result = run_snapshot();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn render_tui_once_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
    *"tmux list-sessions"*)
      printf '%s\n' "claude-1|||1|||0"
      exit 0
      ;;
    *"tmux capture-pane -t 'claude-1'"*)
      printf '%s\n' "Reading file auth.py" "Running tests"
      exit 0
      ;;
  esac
fi
exit 1
"#,
        );

        let rendered = render_tui_once(&azlin, 30, 50).unwrap();

        assert!(rendered.contains("FLEET DASHBOARD"));
        assert!(rendered.contains("[fleet]"));
        assert!(rendered.contains("q quit"));
        assert!(rendered.contains("vm-1"));
        assert!(rendered.contains("claude-1"));
        assert!(rendered.contains("Running tests"));
    }

    #[test]
    fn fleet_tui_ui_state_tracks_selection() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![
                    TmuxSessionInfo {
                        session_name: "claude-1".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::Running,
                        last_output: "first".to_string(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    },
                    TmuxSessionInfo {
                        session_name: "claude-2".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::WaitingInput,
                        last_output: "second".to_string(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    },
                ],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState::default();

        ui_state.sync_to_state(&state);
        assert!(ui_state.selection_matches("vm-1", "claude-1"));

        ui_state.move_selection(&state, 1);
        assert!(ui_state.selection_matches("vm-1", "claude-2"));

        ui_state.move_selection(&state, 1);
        assert!(ui_state.selection_matches("vm-1", "claude-1"));
    }

    #[test]
    fn fleet_tui_ui_state_tracks_only_visible_filtered_sessions() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![
                    TmuxSessionInfo {
                        session_name: "claude-1".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::Running,
                        last_output: "first".to_string(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    },
                    TmuxSessionInfo {
                        session_name: "claude-2".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::WaitingInput,
                        last_output: "second".to_string(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    },
                ],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState {
            status_filter: Some(StatusFilter::Waiting),
            ..Default::default()
        };

        ui_state.sync_to_state(&state);
        assert!(ui_state.selection_matches("vm-1", "claude-2"));

        ui_state.move_selection(&state, 1);
        assert!(ui_state.selection_matches("vm-1", "claude-2"));
    }

    #[test]
    fn fleet_tui_ui_state_tracks_new_session_vm_selection() {
        let state = FleetState {
            vms: vec![
                VmInfo {
                    name: "vm-1".to_string(),
                    session_name: "vm-1".to_string(),
                    os: "linux".to_string(),
                    status: "Running".to_string(),
                    ip: String::new(),
                    region: "westus2".to_string(),
                    tmux_sessions: vec![TmuxSessionInfo {
                        session_name: "claude-1".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::Running,
                        last_output: "first".to_string(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    }],
                },
                VmInfo {
                    name: "vm-2".to_string(),
                    session_name: "vm-2".to_string(),
                    os: "linux".to_string(),
                    status: "Running".to_string(),
                    ip: String::new(),
                    region: "eastus".to_string(),
                    tmux_sessions: Vec::new(),
                },
            ],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::NewSession,
            ..Default::default()
        };

        ui_state.sync_to_state(&state);
        assert_eq!(ui_state.new_session_vm.as_deref(), Some("vm-1"));

        ui_state.move_selection(&state, 1);
        assert_eq!(ui_state.new_session_vm.as_deref(), Some("vm-2"));

        ui_state.move_selection(&state, 1);
        assert_eq!(ui_state.new_session_vm.as_deref(), Some("vm-1"));
    }

    #[test]
    fn fleet_tui_ui_state_can_switch_to_all_sessions_subview() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "eastus".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "copilot-1".to_string(),
                    vm_name: "vm-2".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "awaiting input".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: vec!["vm-2".to_string()],
        };
        let mut ui_state = FleetTuiUiState::default();

        ui_state.sync_to_state(&state);
        assert!(ui_state.selected.is_none());

        ui_state.cycle_fleet_subview(&state);

        assert_eq!(ui_state.fleet_subview, FleetSubview::AllSessions);
        assert!(ui_state.selection_matches("vm-2", "copilot-1"));
        assert_eq!(
            ui_state.status_message.as_deref(),
            Some("Fleet view set to All Sessions.")
        );
    }

    #[test]
    fn render_tui_detail_view_shows_selected_session_and_decision() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 2,
                    attached: true,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "Need instruction\nWaiting for confirmation".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Detail,
            ..Default::default()
        };
        ui_state.sync_to_state(&state);
        ui_state.last_decision = Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "Run cargo test".to_string(),
            reasoning: "Session is waiting on the next command.".to_string(),
            confidence: 0.91,
        });

        let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

        assert!(rendered.contains("[detail]"));
        assert!(rendered.contains("Session Detail"));
        assert!(rendered.contains("Need instruction"));
        assert!(rendered.contains("Prepared proposal"));
        assert!(rendered.contains("Run cargo test"));
    }

    #[test]
    fn load_selected_proposal_into_editor_switches_tabs_and_preserves_input() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "waiting".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState::default();
        ui_state.sync_to_state(&state);
        ui_state.last_decision = Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "y\n".to_string(),
            reasoning: "Needs confirmation.".to_string(),
            confidence: 0.9,
        });

        ui_state.load_selected_proposal_into_editor();

        assert_eq!(ui_state.tab, FleetTuiTab::Editor);
        let editor = ui_state.editor_decision.as_ref().expect("editor decision");
        assert_eq!(editor.input_text, "y\n");
        assert_eq!(editor.action, SessionAction::SendInput);
        assert!(
            ui_state
                .status_message
                .as_deref()
                .is_some_and(|message| message.contains("Loaded proposal into editor"))
        );
    }

    #[test]
    fn load_selected_proposal_into_editor_requires_matching_proposal() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "waiting".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Detail,
            ..Default::default()
        };
        ui_state.sync_to_state(&state);

        ui_state.load_selected_proposal_into_editor();

        assert_eq!(ui_state.tab, FleetTuiTab::Detail);
        assert!(ui_state.editor_decision.is_none());
        assert_eq!(
            ui_state.status_message.as_deref(),
            Some("No prepared proposal for the selected session.")
        );
    }

    #[test]
    fn render_tui_editor_view_shows_prepopulated_decision() {
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Editor,
            ..Default::default()
        };
        ui_state.editor_decision = Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "y\n".to_string(),
            reasoning: "Needs confirmation.".to_string(),
            confidence: 0.9,
        });

        let rendered =
            render_tui_frame(&FleetState::new(PathBuf::from("azlin")), 15, &ui_state).unwrap();

        assert!(rendered.contains("[editor]"));
        assert!(rendered.contains("Action Editor"));
        assert!(rendered.contains("Action: send_input"));
        assert!(rendered.contains("Needs confirmation."));
        assert!(rendered.contains("e reload  i edit input"));
    }

    #[test]
    fn render_tui_fleet_view_shows_placeholder_for_running_vm_without_sessions() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "empty-vm".to_string(),
                session_name: "empty-vm".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus".to_string(),
                tmux_sessions: Vec::new(),
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };

        let rendered = render_tui_frame(&state, 15, &FleetTuiUiState::default()).unwrap();

        assert!(rendered.contains("empty-vm/(no sessions)"));
        // new cockpit renderer shows "empty" label instead of "no tmux sessions detected"
        assert!(rendered.contains("(no sessions)"));
    }

    #[test]
    fn decode_dashboard_key_bytes_handles_arrow_sequences() {
        assert_eq!(
            decode_dashboard_key_bytes(&[0x1b, b'[', b'C']),
            Some(DashboardKey::Right)
        );
        assert_eq!(
            decode_dashboard_key_bytes(&[0x1b, b'[', b'D']),
            Some(DashboardKey::Left)
        );
        assert_eq!(
            decode_dashboard_key_bytes(&[0x1b, b'[', b'A']),
            Some(DashboardKey::Up)
        );
        assert_eq!(
            decode_dashboard_key_bytes(&[0x1b, b'[', b'B']),
            Some(DashboardKey::Down)
        );
        assert_eq!(
            decode_dashboard_key_bytes(&[b'x']),
            Some(DashboardKey::Char('x'))
        );
    }

    #[test]
    fn cycle_tab_helpers_follow_python_tab_order() {
        let mut ui_state = FleetTuiUiState::default();

        assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

        ui_state.cycle_tab_forward();
        assert_eq!(ui_state.tab, FleetTuiTab::Detail);

        ui_state.cycle_tab_forward();
        assert_eq!(ui_state.tab, FleetTuiTab::Projects);

        ui_state.cycle_tab_forward();
        assert_eq!(ui_state.tab, FleetTuiTab::Editor);

        ui_state.cycle_tab_forward();
        assert_eq!(ui_state.tab, FleetTuiTab::NewSession);

        ui_state.cycle_tab_forward();
        assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

        ui_state.cycle_tab_backward();
        assert_eq!(ui_state.tab, FleetTuiTab::NewSession);

        ui_state.cycle_tab_backward();
        assert_eq!(ui_state.tab, FleetTuiTab::Editor);

        ui_state.cycle_tab_backward();
        assert_eq!(ui_state.tab, FleetTuiTab::Projects);

        ui_state.cycle_tab_backward();
        assert_eq!(ui_state.tab, FleetTuiTab::Detail);

        ui_state.cycle_tab_backward();
        assert_eq!(ui_state.tab, FleetTuiTab::Fleet);
    }

    #[test]
    fn toggle_filter_matches_press_again_to_clear_contract() {
        let mut ui_state = FleetTuiUiState::default();

        ui_state.toggle_filter(StatusFilter::Errors);
        assert_eq!(ui_state.status_filter, Some(StatusFilter::Errors));

        ui_state.toggle_filter(StatusFilter::Errors);
        assert_eq!(ui_state.status_filter, None);

        ui_state.toggle_filter(StatusFilter::Waiting);
        assert_eq!(ui_state.status_filter, Some(StatusFilter::Waiting));

        ui_state.toggle_filter(StatusFilter::Active);
        assert_eq!(ui_state.status_filter, Some(StatusFilter::Active));
    }

    #[test]
    fn render_tui_help_overlay_shows_keybinding_reference() {
        let ui_state = FleetTuiUiState {
            show_help: true,
            ..Default::default()
        };
        let rendered =
            render_tui_frame(&FleetState::new(PathBuf::from("azlin")), 15, &ui_state).unwrap();

        assert!(rendered.contains("KEYBINDING HELP"));
        assert!(rendered.contains("1 / f / F"));
        assert!(rendered.contains("5 / n / N"));
        assert!(rendered.contains("Esc / b / B"));
        assert!(rendered.contains("x / X"));
        assert!(rendered.contains("Filters"));
    }

    #[test]
    fn navigate_back_matches_editor_and_detail_hierarchy() {
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Editor,
            ..Default::default()
        };

        ui_state.navigate_back();
        assert_eq!(ui_state.tab, FleetTuiTab::Detail);

        ui_state.navigate_back();
        assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

        ui_state.tab = FleetTuiTab::Projects;
        ui_state.navigate_back();
        assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

        ui_state.tab = FleetTuiTab::NewSession;
        ui_state.navigate_back();
        assert_eq!(ui_state.tab, FleetTuiTab::Fleet);
    }

    #[test]
    fn render_tui_new_session_view_shows_running_vms_and_agent() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: Vec::new(),
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::NewSession,
            ..Default::default()
        };
        ui_state.sync_to_state(&state);

        let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

        assert!(rendered.contains("[new]"));
        assert!(rendered.contains("New Session"));
        assert!(rendered.contains("Agent type: claude"));
        assert!(rendered.contains("> vm-1"));
        assert!(rendered.contains("Enter create"));
    }

    #[test]
    fn render_tui_fleet_view_marks_unmanaged_sessions_in_all_subview() {
        let state = FleetState {
            vms: vec![
                VmInfo {
                    name: "vm-1".to_string(),
                    session_name: "vm-1".to_string(),
                    os: "linux".to_string(),
                    status: "Running".to_string(),
                    ip: String::new(),
                    region: "westus2".to_string(),
                    tmux_sessions: vec![TmuxSessionInfo {
                        session_name: "claude-1".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::Running,
                        last_output: "shipping".to_string(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    }],
                },
                VmInfo {
                    name: "vm-2".to_string(),
                    session_name: "vm-2".to_string(),
                    os: "linux".to_string(),
                    status: "Running".to_string(),
                    ip: String::new(),
                    region: "eastus".to_string(),
                    tmux_sessions: vec![TmuxSessionInfo {
                        session_name: "copilot-1".to_string(),
                        vm_name: "vm-2".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::WaitingInput,
                        last_output: "waiting".to_string(),
                        working_directory: String::new(),
                        repo_url: String::new(),
                        git_branch: String::new(),
                        pr_url: String::new(),
                        task_summary: String::new(),
                    }],
                },
            ],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: vec!["vm-2".to_string()],
        };
        let mut ui_state = FleetTuiUiState {
            fleet_subview: FleetSubview::AllSessions,
            ..Default::default()
        };
        ui_state.sync_to_state(&state);

        let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

        assert!(rendered.contains("[all]"));
        assert!(rendered.contains("All Sessions"));
        assert!(rendered.contains("vm-2"));
        assert!(rendered.contains("unmanaged"));
        assert!(rendered.contains("copilot-1"));
    }

    #[test]
    fn render_tui_fleet_view_shows_selected_session_preview() {
        let state = FleetState {
            vms: vec![
                VmInfo {
                    name: "vm-1".to_string(),
                    session_name: "vm-1".to_string(),
                    os: "linux".to_string(),
                    status: "Running".to_string(),
                    ip: String::new(),
                    region: "westus2".to_string(),
                    tmux_sessions: vec![TmuxSessionInfo {
                        session_name: "claude-1".to_string(),
                        vm_name: "vm-1".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::Running,
                        last_output: "build ok".to_string(),
                        working_directory: "/tmp/demo".to_string(),
                        repo_url: "https://github.com/org/demo.git".to_string(),
                        git_branch: "main".to_string(),
                        pr_url: String::new(),
                        task_summary: "Build is passing".to_string(),
                    }],
                },
                VmInfo {
                    name: "vm-2".to_string(),
                    session_name: "vm-2".to_string(),
                    os: "linux".to_string(),
                    status: "Running".to_string(),
                    ip: String::new(),
                    region: "eastus".to_string(),
                    tmux_sessions: vec![TmuxSessionInfo {
                        session_name: "copilot-1".to_string(),
                        vm_name: "vm-2".to_string(),
                        windows: 1,
                        attached: false,
                        agent_status: AgentStatus::WaitingInput,
                        last_output: "review queued\nneed human ack".to_string(),
                        working_directory: "/tmp/excluded".to_string(),
                        repo_url: "https://github.com/org/excluded.git".to_string(),
                        git_branch: "side-quest".to_string(),
                        pr_url: "https://github.com/org/excluded/pull/7".to_string(),
                        task_summary: "Waiting for operator review".to_string(),
                    }],
                },
            ],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: vec!["vm-2".to_string()],
        };
        let mut ui_state = FleetTuiUiState {
            fleet_subview: FleetSubview::AllSessions,
            ..Default::default()
        };
        ui_state.sync_to_state(&state);
        ui_state.move_selection(&state, 1);

        let rendered = render_tui_frame(&state, 18, &ui_state).unwrap();

        assert!(rendered.contains("Selected session: vm-2/copilot-1"));
        assert!(rendered.contains("branch: side-quest"));
        assert!(rendered.contains("repo: https://github.com/org/excluded.git"));
        assert!(rendered.contains("cwd: /tmp/excluded"));
        assert!(rendered.contains("task: Waiting for operator review"));
        assert!(rendered.contains("need human ack"));
    }

    #[test]
    fn skip_selected_proposal_clears_matching_detail_and_editor_state() {
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "waiting".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let decision = SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "y\n".to_string(),
            reasoning: "Needs confirmation.".to_string(),
            confidence: 0.9,
        };
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Editor,
            last_decision: Some(decision.clone()),
            editor_decision: Some(decision),
            ..Default::default()
        };
        ui_state.sync_to_state(&state);

        ui_state.skip_selected_proposal();

        assert_eq!(ui_state.tab, FleetTuiTab::Detail);
        assert!(ui_state.last_decision.is_none());
        assert!(ui_state.editor_decision.is_none());
        assert_eq!(ui_state.status_message.as_deref(), Some("Skipped."));
    }

    #[test]
    fn run_tui_apply_edited_returns_to_detail_on_success() {
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Editor,
            ..Default::default()
        };
        ui_state.editor_decision = Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::Wait,
            input_text: String::new(),
            reasoning: "testing apply".to_string(),
            confidence: 1.0,
        });

        let result = run_tui_apply_edited(Path::new("azlin"), &mut ui_state);

        assert!(result.is_ok());
        assert_eq!(ui_state.tab, FleetTuiTab::Detail);
        assert!(
            ui_state
                .status_message
                .as_deref()
                .is_some_and(|message| message.contains("Applied edited wait"))
        );
    }

    #[test]
    fn run_tui_apply_reports_missing_prepared_proposal() {
        let mut ui_state = FleetTuiUiState::default();

        let result = run_tui_apply(Path::new("azlin"), &mut ui_state);

        assert!(result.is_ok());
        assert_eq!(
            ui_state.status_message.as_deref(),
            Some("No prepared proposal to apply.")
        );
    }

    #[test]
    fn run_tui_apply_reports_dangerous_input_block() {
        let mut ui_state = FleetTuiUiState::default();
        ui_state.last_decision = Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "rm -rf /".to_string(),
            reasoning: "testing dangerous input".to_string(),
            confidence: 0.95,
        });

        let result = run_tui_apply(Path::new("azlin"), &mut ui_state);

        assert!(result.is_ok());
        assert!(
            ui_state
                .status_message
                .as_deref()
                .is_some_and(|message| message.contains("dangerous-input policy"))
        );
    }

    #[test]
    fn run_tui_apply_edited_reports_dangerous_input_block() {
        let mut ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Editor,
            ..Default::default()
        };
        ui_state.editor_decision = Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "rm -rf /".to_string(),
            reasoning: "testing dangerous input".to_string(),
            confidence: 0.95,
        });

        let result = run_tui_apply_edited(Path::new("azlin"), &mut ui_state);

        assert!(result.is_ok());
        assert!(
            ui_state
                .status_message
                .as_deref()
                .is_some_and(|message| message.contains("dangerous-input policy"))
        );
    }

    #[test]
    fn run_tui_adopt_selected_session_adds_running_task() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  printf '%s\n' \
    "===SESSION:work-1===" \
    "CWD:/workspace/repo" \
    "CMD:claude" \
    "REPO:https://github.com/org/repo.git" \
    "BRANCH:feat/login" \
    "LAST_MSG:Resume work on auth" \
    "===DONE==="
  exit 0
fi
exit 1
"#,
        );

        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "work-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "Working".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: azlin.clone(),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState::default();
        ui_state.sync_to_state(&state);

        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("HOME", home.path()) };
        let result = run_tui_adopt_selected_session(&azlin, &state, &mut ui_state);
        let queue =
            TaskQueue::load(Some(home.path().join(".amplihack/fleet/task_queue.json"))).unwrap();
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(queue.tasks[0].status, TaskStatus::Running);
        assert_eq!(queue.tasks[0].assigned_vm.as_deref(), Some("vm-1"));
        assert_eq!(queue.tasks[0].assigned_session.as_deref(), Some("work-1"));
        assert_eq!(
            ui_state.status_message.as_deref(),
            Some("Adopted vm-1/work-1 into the fleet queue.")
        );
    }

    #[test]
    fn run_tui_adopt_selected_session_rejects_duplicate_active_assignment() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let state = FleetState {
            vms: vec![VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "work-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "Working".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            }],
            timestamp: None,
            azlin_path: PathBuf::from("azlin"),
            exclude_vms: Vec::new(),
        };
        let mut ui_state = FleetTuiUiState::default();
        ui_state.sync_to_state(&state);

        let queue_path = home.path().join(".amplihack/fleet/task_queue.json");
        let mut queue = TaskQueue {
            tasks: Vec::new(),
            persist_path: Some(queue_path.clone()),
            load_failed: false,
        };
        let mut task = FleetTask::new(
            "Resume work on auth",
            "https://github.com/org/repo.git",
            TaskPriority::Medium,
            "claude",
            "auto",
            DEFAULT_MAX_TURNS,
        );
        task.status = TaskStatus::Running;
        task.assigned_vm = Some("vm-1".to_string());
        task.assigned_session = Some("work-1".to_string());
        queue.tasks.push(task);
        queue.save().unwrap();

        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("HOME", home.path()) };
        let result = run_tui_adopt_selected_session(Path::new("azlin"), &state, &mut ui_state);
        let reloaded = TaskQueue::load(Some(queue_path)).unwrap();
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
        assert_eq!(reloaded.tasks.len(), 1);
        assert_eq!(
            ui_state.status_message.as_deref(),
            Some("vm-1/work-1 is already adopted into the active fleet queue.")
        );
    }

    #[test]
    fn render_tui_projects_view_uses_dashboard_file() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("HOME", home.path()) };
        let dashboard_path = home.path().join(".amplihack/fleet/dashboard.json");
        fs::create_dir_all(dashboard_path.parent().unwrap()).unwrap();
        fs::write(
            &dashboard_path,
            serde_json::json!([{
                    "repo_url": "https://github.com/org/repo",
                    "name": "repo",
                    "github_identity": "bot",
                    "priority": "high",
                    "notes": "Important",
                    "vms": ["vm-1"],
                    "tasks_total": 3,
                    "tasks_completed": 2,
                    "tasks_failed": 0,
                    "tasks_in_progress": 1,
                    "prs_created": ["https://github.com/org/repo/pull/1"],
                    "estimated_cost_usd": 1.0,
                    "started_at": now_isoformat(),
                    "last_activity": now_isoformat()
                }]
            )
            .to_string(),
        )
        .unwrap();

        let state = FleetState::new(PathBuf::from("azlin"));
        let ui_state = FleetTuiUiState {
            tab: FleetTuiTab::Projects,
            ..Default::default()
        };
        let rendered = render_tui_frame(&state, 20, &ui_state).unwrap();

        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(rendered.contains("[projects]"));
        assert!(rendered.contains("Fleet Projects (1)"));
        assert!(rendered.contains("https://github.com/org/repo"));
    }

    #[test]
    fn run_auth_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "cp" ]; then
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
    "mkdir -p '~/.config/gh'"|"chmod 600 '~/.config/gh/hosts.yml'"|"chmod 600 '~/.config/gh/config.yml'")
      exit 0
      ;;
    "gh auth status")
      exit 0
      ;;
    "az account show --query name -o tsv")
      exit 1
      ;;
  esac
fi
exit 1
"#,
        );

        fs::create_dir_all(home.path().join(".config/gh")).unwrap();
        fs::write(home.path().join(".config/gh/hosts.yml"), "hosts").unwrap();
        fs::write(home.path().join(".config/gh/config.yml"), "config").unwrap();

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("AZLIN_PATH", &azlin) };
        unsafe { env::set_var("HOME", home.path()) };

        let result = run_auth("vm-1", &[String::from("github")]);

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn run_adopt_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  printf '%s\n' \
    "===SESSION:work-1===" \
    "CWD:/workspace/repo" \
    "CMD:claude" \
    "REPO:https://github.com/org/repo.git" \
    "BRANCH:feat/login" \
    "===DONE==="
  exit 0
fi
exit 1
"#,
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("AZLIN_PATH", &azlin) };
        unsafe { env::set_var("HOME", home.path()) };

        let result = run_adopt("vm-1", &[]);
        let queue =
            TaskQueue::load(Some(home.path().join(".amplihack/fleet/task_queue.json"))).unwrap();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(queue.tasks[0].status, TaskStatus::Running);
        assert_eq!(queue.tasks[0].assigned_vm.as_deref(), Some("vm-1"));
        assert_eq!(queue.tasks[0].assigned_session.as_deref(), Some("work-1"));
    }

    #[test]
    fn run_report_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
    *"tmux list-sessions"*)
      printf '%s\n' "claude-1|||1|||0"
      exit 0
      ;;
    *"tmux capture-pane -t 'claude-1'"*)
      printf '%s\n' "Step 5: Implementing auth"
      exit 0
      ;;
  esac
fi
exit 1
"#,
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("AZLIN_PATH", &azlin) };
        unsafe { env::set_var("HOME", home.path()) };

        let result = run_report();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn fleet_admiral_reason_emits_lifecycle_and_batch_actions() {
        let temp = tempfile::tempdir().unwrap();
        let mut admiral = FleetAdmiral::new(
            PathBuf::from("/bin/true"),
            TaskQueue {
                tasks: vec![
                    FleetTask {
                        id: "done-task".to_string(),
                        prompt: "Finish feature".to_string(),
                        repo_url: "https://github.com/org/repo.git".to_string(),
                        branch: String::new(),
                        priority: TaskPriority::High,
                        status: TaskStatus::Running,
                        agent_command: "claude".to_string(),
                        agent_mode: "auto".to_string(),
                        max_turns: DEFAULT_MAX_TURNS,
                        protected: false,
                        assigned_vm: Some("vm-1".to_string()),
                        assigned_session: Some("session-1".to_string()),
                        assigned_at: Some(now_isoformat()),
                        created_at: now_isoformat(),
                        started_at: Some(now_isoformat()),
                        completed_at: None,
                        result: None,
                        pr_url: None,
                        error: None,
                    },
                    FleetTask {
                        id: "queued-task".to_string(),
                        prompt: "Implement auth".to_string(),
                        repo_url: "https://github.com/org/repo.git".to_string(),
                        branch: String::new(),
                        priority: TaskPriority::Medium,
                        status: TaskStatus::Queued,
                        agent_command: "claude".to_string(),
                        agent_mode: "auto".to_string(),
                        max_turns: DEFAULT_MAX_TURNS,
                        protected: false,
                        assigned_vm: None,
                        assigned_session: None,
                        assigned_at: None,
                        created_at: now_isoformat(),
                        started_at: None,
                        completed_at: None,
                        result: None,
                        pr_url: None,
                        error: None,
                    },
                ],
                persist_path: None,
                load_failed: false,
            },
            Some(temp.path().join("logs")),
        )
        .unwrap();
        admiral.coordination_dir = temp.path().join("coordination");
        admiral.fleet_state.vms = vec![
            VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "ubuntu".to_string(),
                status: "Running".to_string(),
                ip: "10.0.0.1".to_string(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "session-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Completed,
                    last_output: "PR created".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
            VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: "ubuntu".to_string(),
                status: "Running".to_string(),
                ip: "10.0.0.2".to_string(),
                region: "westus2".to_string(),
                tmux_sessions: vec![],
            },
        ];

        let actions = admiral.reason().unwrap();

        assert!(
            actions
                .iter()
                .any(|action| action.action_type == ActionType::MarkComplete)
        );
        assert!(
            actions
                .iter()
                .any(|action| action.action_type == ActionType::StartAgent)
        );
    }

    #[test]
    fn fleet_admiral_start_agent_updates_task_state_and_log() {
        let temp = tempfile::tempdir().unwrap();
        let azlin = temp.path().join("azlin");
        write_executable(&azlin, "#!/bin/sh\nexit 0\n");

        let task = FleetTask {
            id: "queued-task".to_string(),
            prompt: "Implement auth".to_string(),
            repo_url: String::new(),
            branch: String::new(),
            priority: TaskPriority::Medium,
            status: TaskStatus::Queued,
            agent_command: "claude".to_string(),
            agent_mode: "auto".to_string(),
            max_turns: DEFAULT_MAX_TURNS,
            protected: false,
            assigned_vm: None,
            assigned_session: None,
            assigned_at: None,
            created_at: now_isoformat(),
            started_at: None,
            completed_at: None,
            result: None,
            pr_url: None,
            error: None,
        };
        let mut admiral = FleetAdmiral::new(
            azlin,
            TaskQueue {
                tasks: vec![task.clone()],
                persist_path: Some(temp.path().join("task_queue.json")),
                load_failed: false,
            },
            Some(temp.path().join("logs")),
        )
        .unwrap();
        admiral.coordination_dir = temp.path().join("coordination");

        let action = DirectorAction::new(
            ActionType::StartAgent,
            Some(task),
            Some("vm-1".to_string()),
            Some("fleet-queued-task".to_string()),
            "Batch assign: MEDIUM task",
        );

        let results = admiral.act(&[action]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "Agent started: fleet-queued-task on vm-1");
        let saved = &admiral.task_queue.tasks[0];
        assert_eq!(saved.status, TaskStatus::Running);
        assert_eq!(saved.assigned_vm.as_deref(), Some("vm-1"));
        assert_eq!(saved.assigned_session.as_deref(), Some("fleet-queued-task"));
        assert!(temp.path().join("logs/admiral_log.json").exists());
    }

    #[test]
    fn run_observe_returns_exit_error_when_vm_missing() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
exit 1
"#,
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("AZLIN_PATH", &azlin) };
        unsafe { env::set_var("HOME", home.path()) };

        let error = run_observe("missing-vm").unwrap_err();

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert_eq!(command_error::exit_code(&error), Some(1));
    }

    #[test]
    fn run_observe_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
    *"tmux list-sessions"*)
      printf '%s\n' "claude-1|||1|||0"
      exit 0
      ;;
    *"tmux capture-pane -t 'claude-1'"*)
      printf '%s\n' "Step 5: Implementing auth" "Reading file auth.py" "Running tests"
      exit 0
      ;;
  esac
fi
exit 1
"#,
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_home = env::var_os("HOME");
        unsafe { env::set_var("AZLIN_PATH", &azlin) };
        unsafe { env::set_var("HOME", home.path()) };

        let result = run_observe("vm-1");

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn run_watch_succeeds_with_stubbed_azlin() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            "#!/bin/sh\nif [ \"$1\" = connect ]; then\n  printf \"agent output line 1\\nagent output line 2\\n\";\nelse\n  exit 1\nfi\n",
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("AZLIN_PATH", &azlin);
            env::set_var("PATH", dir.path());
            env::set_var("HOME", home.path());
        }

        let result = run_watch("test-vm", "session-1", 30);

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn run_watch_failure_is_nonfatal() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(
            &azlin,
            "#!/bin/sh\nprintf 'connection refused' >&2\nexit 1\n",
        );

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("AZLIN_PATH", &azlin);
            env::set_var("PATH", dir.path());
            env::set_var("HOME", home.path());
        }

        let result = run_watch("test-vm", "session-1", 30);

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn run_watch_timeout_is_nonfatal() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let azlin = dir.path().join("azlin");
        write_executable(&azlin, "#!/bin/sh\nsleep 1\n");

        let previous_azlin = env::var_os("AZLIN_PATH");
        let previous_path = env::var_os("PATH");
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("AZLIN_PATH", &azlin);
            env::set_var("PATH", dir.path());
            env::set_var("HOME", home.path());
        }

        let result = run_watch_with_timeout("test-vm", "session-1", 30, Duration::from_secs(0));

        match previous_azlin {
            Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
            None => unsafe { env::remove_var("AZLIN_PATH") },
        }
        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        assert!(result.is_ok());
    }

    #[test]
    fn run_watch_rejects_invalid_vm_name() {
        let err = run_watch("bad vm!@#", "session-1", 30).expect_err("invalid VM should fail");
        assert_eq!(command_error::exit_code(&err), None);
        assert!(err.to_string().contains("Invalid VM name"));
    }
}
