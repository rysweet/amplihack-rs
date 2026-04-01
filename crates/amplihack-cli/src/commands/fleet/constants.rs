use std::env;
use std::num::NonZeroUsize;
use std::time::Duration;

pub(crate) const FLEET_EXISTING_VMS: &[&str] = &[];
pub(crate) const FLEET_EXISTING_VMS_ENV: &str = "AMPLIHACK_FLEET_EXISTING_VMS";
pub(crate) const AZLIN_VERSION_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const AZLIN_LIST_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const TMUX_LIST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const CLI_WATCH_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) const DEFAULT_MAX_TURNS: u32 = 20;
pub(crate) const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 60;
pub(crate) const DEFAULT_DASHBOARD_REFRESH_SECONDS: u64 = 30;
pub(crate) const DEFAULT_MAX_AGENTS_PER_VM: usize = 3;
pub(crate) const DEFAULT_CAPTURE_LINES: usize = 50;
pub(crate) const MAX_CAPTURE_LINES: usize = 10_000;
pub(crate) const DEFAULT_STUCK_THRESHOLD_SECONDS: f64 = 300.0;
pub(crate) const CONFIDENCE_COMPLETION: f64 = 0.9;
pub(crate) const CONFIDENCE_ERROR: f64 = 0.85;
pub(crate) const CONFIDENCE_THINKING: f64 = 1.0;
pub(crate) const CONFIDENCE_RUNNING: f64 = 0.8;

pub(crate) fn configured_existing_vms() -> Vec<String> {
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
pub(crate) const CONFIDENCE_IDLE: f64 = 0.7;
pub(crate) const CONFIDENCE_DEFAULT_RUNNING: f64 = 0.5;
pub(crate) const CONFIDENCE_UNKNOWN: f64 = 0.3;
pub(crate) const MIN_CONFIDENCE_SEND: f64 = 0.6;
pub(crate) const MIN_CONFIDENCE_RESTART: f64 = 0.8;
pub(crate) const MIN_SUBSTANTIAL_OUTPUT_LEN: usize = 50;
pub(crate) const SCOUT_REASONER_TIMEOUT: Duration = Duration::from_secs(180);
// T5: LRU capture cache capacity (64 entries — one per session).
pub(crate) const CAPTURE_CACHE_CAPACITY: usize = 64;
pub(crate) const CAPTURE_CACHE_CAPACITY_NONZERO: NonZeroUsize =
    NonZeroUsize::new(CAPTURE_CACHE_CAPACITY).expect("cache capacity must be nonzero");
// T4: Two-phase background refresh timing (verified against _tui_refresh.py).
// Python uses set_interval("refresh", 5) for slow phase and 0.5s for fast.
pub(crate) const TUI_FAST_REFRESH_INTERVAL_MS: u64 = 500;
pub(crate) const TUI_SLOW_REFRESH_INTERVAL_MS: u64 = 5_000;
pub(crate) const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
pub(crate) const HIDE_CURSOR: &str = "\x1b[?25l";
pub(crate) const SHOW_CURSOR: &str = "\x1b[?25h";

// ANSI color/style codes for the fleet cockpit renderer.
pub(crate) const ANSI_RESET: &str = "\x1b[0m";
pub(crate) const ANSI_BOLD: &str = "\x1b[1m";
pub(crate) const ANSI_DIM: &str = "\x1b[2m";
pub(crate) const ANSI_GREEN: &str = "\x1b[32m";
pub(crate) const ANSI_YELLOW: &str = "\x1b[33m";
pub(crate) const ANSI_RED: &str = "\x1b[31m";
pub(crate) const ANSI_BLUE: &str = "\x1b[34m";
pub(crate) const ANSI_CYAN: &str = "\x1b[36m";

// Unicode box-drawing characters for the fleet cockpit border.
pub(crate) const BOX_TL: char = '\u{2554}'; // ╔ top-left double
pub(crate) const BOX_TR: char = '\u{2557}'; // ╗ top-right double
pub(crate) const BOX_BL: char = '\u{255a}'; // ╚ bottom-left double
pub(crate) const BOX_BR: char = '\u{255d}'; // ╝ bottom-right double
pub(crate) const BOX_HL: char = '\u{2550}'; // ═ horizontal double
pub(crate) const BOX_VL: char = '\u{2551}'; // ║ vertical double
pub(crate) const BOX_ML: char = '\u{2560}'; // ╠ middle-left junction
pub(crate) const BOX_MR: char = '\u{2563}'; // ╣ middle-right junction
pub(crate) const BOX_DASH: char = '\u{2500}'; // ─ thin horizontal (VM section separator)

pub(crate) const SESSION_REASONER_SYSTEM_PROMPT: &str = r#"You are a Fleet Admiral managing coding agent sessions across multiple VMs.

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

pub(crate) const COMPLETION_PATTERNS: &[&str] = &[
    r"PR.*created",
    r"pull request.*created",
    r"GOAL_STATUS:\s*ACHIEVED",
    r"Workflow Complete",
    r"All \d+ steps completed",
    r"pushed to.*branch",
];
pub(crate) const ERROR_PATTERNS: &[&str] = &[
    r"(?:^|\n)\s*(?:ERROR|FATAL|CRITICAL):",
    r"Traceback \(most recent",
    r"panic:",
    r"GOAL_STATUS:\s*NOT_ACHIEVED",
    r"Permission denied",
    r"Authentication failed",
];
pub(crate) const WAITING_PATTERNS: &[&str] = &[
    r"[?]\s*\[Y/n\]",
    r"[?]\s*\[y/N\]",
    r"\(yes/no\)",
    r"Press .* to continue",
    r"Do you want to",
    r"^Enter\s+\w+\s*:",
    r"waiting for.*input",
];
pub(crate) const RUNNING_PATTERNS: &[&str] = &[
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
pub(crate) const IDLE_PATTERNS: &[&str] = &[r"\$\s*$", r"azureuser@.*:\~.*\$", r"❯\s*$"];
pub(crate) const SAFE_INPUT_PATTERNS: &[&str] = &[
    r"^[yYnN]$",
    r"^(yes|no)$",
    r"^/[a-z]",
    r"^(exit|quit|q)$",
    r"^\d+$",
    r"^(git status|git log|git diff|git branch)",
    r"^(ls|pwd|wc|which)\b",
    r"^(pytest|make|npm test|npm run|cargo test)",
];
pub(crate) const DANGEROUS_INPUT_PATTERNS: &[&str] = &[
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
pub(crate) const AUTH_GITHUB_FILES: &[(&str, &str, &str)] = &[
    ("~/.config/gh/hosts.yml", "~/.config/gh/hosts.yml", "600"),
    ("~/.config/gh/config.yml", "~/.config/gh/config.yml", "600"),
];
pub(crate) const AUTH_AZURE_FILES: &[(&str, &str, &str)] = &[
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
pub(crate) const AUTH_CLAUDE_FILES: &[(&str, &str, &str)] = &[("~/.claude.json", "~/.claude.json", "600")];
