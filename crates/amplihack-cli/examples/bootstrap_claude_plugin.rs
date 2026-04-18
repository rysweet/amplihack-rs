//! One-shot harness: install amplihack as a Claude Code plugin.
//!
//! Run with `cargo run --example bootstrap_claude_plugin -p amplihack-cli`.
//! Used to exercise the plugin install path without launching Claude Code
//! (the normal trigger is inside `bootstrap::prepare_launcher("claude", ...)`).

fn main() -> anyhow::Result<()> {
    amplihack_cli::claude_plugin::ensure_claude_plugin_installed()?;
    println!("✅ amplihack Claude Code plugin bootstrap complete");
    Ok(())
}
