//! User-facing display helpers (port of `display.py`).
//!
//! Pure formatting functions that return strings rather than printing
//! directly, to keep them testable and reusable from the CLI layer.

use crate::security::{create_safe_preview, filter_pattern_suggestion};

/// Returns true when reflection output should be visible (env-driven).
pub fn should_show_output() -> bool {
    match std::env::var("REFLECTION_VISIBILITY") {
        Ok(v) => {
            let v = v.to_ascii_lowercase();
            !matches!(v.as_str(), "false" | "0" | "no" | "off")
        }
        Err(_) => true,
    }
}

pub fn format_analysis_start(message_count: usize) -> String {
    let bar = "=".repeat(50);
    format!(
        "\n{bar}\n🤖 AI REFLECTION ANALYSIS STARTING\n📊 Analyzing {message_count} messages for improvements...\n{bar}",
    )
}

pub fn format_pattern_found(pattern_type: &str, suggestion: &str, priority: &str) -> String {
    let safe = filter_pattern_suggestion(suggestion);
    format!("🎯 Found {priority} priority {pattern_type}: {safe}")
}

pub fn format_issue_created(issue_url: &str, pattern_type: &str) -> String {
    let issue_number = issue_url.rsplit('/').next().unwrap_or("unknown");
    format!(
        "✅ Created GitHub issue #{issue_number} for {pattern_type} improvement\n📎 {issue_url}",
    )
}

pub fn format_automation_status(issue_number: &str, success: bool) -> String {
    if success {
        format!("🚀 UltraThink will create PR for issue #{issue_number}")
    } else {
        format!("⚠️  Manual follow-up needed for issue #{issue_number}")
    }
}

pub fn format_analysis_complete(patterns_found: usize, issues_created: usize) -> String {
    let bar = "=".repeat(50);
    let mut out = format!(
        "\n{bar}\n🏁 REFLECTION ANALYSIS COMPLETE\n📊 Found {patterns_found} improvement opportunities",
    );
    if issues_created > 0 {
        out.push_str(&format!("\n🎫 Created {issues_created} GitHub issue(s)"));
    }
    out.push_str(&format!("\n{bar}\n"));
    out
}

pub fn format_error(error_msg: &str) -> String {
    let safe = create_safe_preview(error_msg, "Error");
    format!("❌ REFLECTION ERROR: {safe}")
}
