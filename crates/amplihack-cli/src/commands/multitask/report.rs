//! End-of-run reporting for the parallel workstream orchestrator.

use super::models::Workstream;
use std::fs;
use std::path::Path;
use tracing::warn;

/// The end-of-run report, and the copy of it written to `REPORT.md`.
///
/// A brick of its own: `orchestrator.rs` sits against the 400-line limit that
/// `multitask_orchestrator_spec_test` enforces, and string assembly has no
/// business competing with the launch and monitor paths for that budget.
pub(super) fn render(
    mode: &str,
    base_dir: &Path,
    workstreams: &[Workstream],
    cleaned_up: usize,
    freed_bytes: u64,
) -> String {
    let mut lines = vec![
        String::new(),
        "=".repeat(70),
        "PARALLEL WORKSTREAM REPORT".to_string(),
        format!("Mode: {mode}"),
        "=".repeat(70),
    ];

    let mut succeeded = 0u32;
    let mut failed = 0u32;

    for ws in workstreams {
        let runtime = ws
            .runtime_seconds()
            .map(|s| format!("{s:.0}s"))
            .unwrap_or_else(|| "N/A".to_string());

        let status = if ws.exit_code == Some(0) {
            succeeded += 1;
            "OK".to_string()
        } else {
            failed += 1;
            format!(
                "FAILED (exit {})",
                ws.exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".to_string())
            )
        };

        lines.push(format!("\n[{}] {}", ws.key, ws.description));
        lines.push(format!("  Branch:    {}", ws.branch));
        lines.push(format!("  Status:    {status}"));
        lines.push(format!("  Lifecycle: {}", ws.lifecycle_state));
        lines.push(format!("  Runtime:   {runtime}"));
        lines.push(format!(
            "  Checkpoint: {}",
            if ws.checkpoint_id.is_empty() {
                "n/a"
            } else {
                &ws.checkpoint_id
            }
        ));
        lines.push(format!(
            "  Worktree: {}",
            if ws.worktree_path.is_empty() {
                "n/a"
            } else {
                &ws.worktree_path
            }
        ));
        lines.push(format!("  Log:       {}", ws.log_file.display()));
        lines.push(format!("  Cleanup eligible: {}", ws.cleanup_eligible));
    }

    let freed_gb = freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    lines.push(String::new());
    lines.push("-".repeat(70));
    lines.push(format!(
        "Total: {} | Succeeded: {succeeded} | Failed: {failed}",
        workstreams.len()
    ));
    lines.push(String::new());
    lines.push("DISK MANAGEMENT:".to_string());
    lines.push(format!(
        "  Auto-cleaned: {cleaned_up} workstream dirs ({freed_gb:.2}GB freed)",
    ));
    lines.push(format!(
        "  Log files preserved at: {}/log-*.txt",
        base_dir.display()
    ));
    lines.push("=".repeat(70));

    let report_text = lines.join("\n");

    // Write report to file
    let report_file = base_dir.join("REPORT.md");
    if let Err(e) = fs::write(&report_file, &report_text) {
        warn!("Failed to write report to {}: {e}", report_file.display());
    } else {
        println!("Report saved to: {}", report_file.display());
    }

    report_text
}
