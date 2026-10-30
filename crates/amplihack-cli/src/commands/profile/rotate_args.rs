//! The `profile rotate` command line: its subcommands and tuning flags.
//! Split from `rotate/mod.rs`, which re-exports them; built on every
//! platform, as the command line is.

/// `profile rotate`: one cycle, or a subcommand. Tuning flags of `start` and
/// `watch` go after the subcommand.
#[derive(Debug, clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RotateArgs {
    #[command(subcommand)]
    pub action: Option<RotateAction>,
    /// Measure and decide, print what would happen, and change nothing
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub tune: TuneFlags,
}

#[derive(Debug, clap::Subcommand)]
pub enum RotateAction {
    /// Start `rotate watch` as a detached background process
    Start(WatchFlags),
    /// Run a cycle every --interval seconds in the foreground
    Watch(WatchFlags),
    /// Stop the background rotator
    Stop,
    /// Show whether the rotator runs, the live profile and the last cycle
    Status,
    /// Show, or set, whether launches start the rotator while auto-switch is on
    Autostart {
        #[arg(value_parser = ["on", "off"])]
        state: Option<String>,
    },
}

/// Thresholds and margins. Kept as text so the error can name the flag; see
/// `settings::resolve`.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct TuneFlags {
    /// Swap when the live session usage reaches N% (default 85)
    #[arg(long, value_name = "N")]
    pub session_at: Option<String>,
    /// Swap when the live weekly usage reaches N% (default 90)
    #[arg(long, value_name = "N")]
    pub week_at: Option<String>,
    /// Points of headroom the target needs over the live login (default 5)
    #[arg(long, value_name = "N")]
    pub margin: Option<String>,
    /// Points of headroom that justify a swap below the thresholds (default 30)
    #[arg(long, value_name = "N")]
    pub upgrade_margin: Option<String>,
    /// Seconds after a swap during which no upgrade happens; 0 disables (default 3600)
    #[arg(long, value_name = "SECS")]
    pub hold: Option<String>,
    /// Move onto an overflow profile only when the earliest plan reset is more than SECS away; 0 means at once (default 1800)
    #[arg(long, value_name = "SECS")]
    pub overflow_wait: Option<String>,
    /// 1: a plan reset no reading reports counts as far away, so an unknown reset can move onto an overflow profile and spend money; 0: it never does (default 0)
    #[arg(long, value_name = "0|1")]
    pub overflow_unknown_reset: Option<String>,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct WatchFlags {
    #[command(flatten)]
    pub tune: TuneFlags,
    /// Seconds between cycles (default 600)
    #[arg(long, value_name = "SECS")]
    pub interval: Option<String>,
    /// Set by `start`: the pid file is already written
    #[arg(long, hide = true)]
    pub spawned: bool,
}
