//! CodexTranscriptsBuilder — aggregates Codex session transcripts into a
//! comprehensive corpus, focused excerpts, and insight reports.
//!
//! Native Rust port of `codex_transcripts_builder.py`. The Python module
//! supports many output formats; the Rust port implements the subset
//! exercised by the test contract (`build_comprehensive_codex`,
//! `build_focused_codex`, `extract_learning_corpus`,
//! `generate_insights_report`).

use std::path::{Path, PathBuf};

use anyhow::Context;

use super::parser::{CodexSession, parse_session};
use super::serializer::{render_assistant_corpus, render_insights_markdown, render_session_block};

pub struct CodexTranscriptsBuilder {
    output_dir: Option<PathBuf>,
}

impl CodexTranscriptsBuilder {
    pub fn new(output_dir: Option<PathBuf>) -> Self {
        Self { output_dir }
    }

    pub fn output_dir(&self) -> Option<&Path> {
        self.output_dir.as_deref()
    }

    /// Build comprehensive codex from all session JSON files under
    /// `output_dir` (recursively walks one level of subdirectories).
    pub fn build_comprehensive_codex(
        &self,
        session_filter: Option<Vec<String>>,
    ) -> anyhow::Result<String> {
        let sessions = self.load_sessions(session_filter.as_deref())?;
        if sessions.is_empty() {
            return Ok(String::new());
        }
        let mut out = String::from("# Comprehensive Codex\n\n");
        for s in &sessions {
            out.push_str(&render_session_block(s));
        }
        Ok(out)
    }

    /// Build a focused excerpt that contains messages whose plain-text body
    /// mentions `focus_area` (case-insensitive).
    pub fn build_focused_codex(
        &self,
        focus_area: &str,
        session_filter: Option<Vec<String>>,
    ) -> anyhow::Result<String> {
        let needle = focus_area.to_ascii_lowercase();
        let sessions = self.load_sessions(session_filter.as_deref())?;
        let mut out = format!("# Focused Codex: {focus_area}\n\n");
        for s in &sessions {
            let mut hit = false;
            for m in &s.messages {
                let body = m.content.as_plain_text();
                if body.to_ascii_lowercase().contains(&needle) {
                    if !hit {
                        out.push_str(&format!("## Session: {}\n\n", s.session_id));
                        hit = true;
                    }
                    out.push_str(&format!("- **{}**: {body}\n", m.role));
                }
            }
        }
        Ok(out)
    }

    /// Concatenate assistant messages across sessions for downstream
    /// learning/embedding pipelines.
    pub fn extract_learning_corpus(
        &self,
        session_filter: Option<Vec<String>>,
    ) -> anyhow::Result<String> {
        let sessions = self.load_sessions(session_filter.as_deref())?;
        Ok(render_assistant_corpus(&sessions))
    }

    /// Generate a markdown insights report.
    pub fn generate_insights_report(
        &self,
        session_filter: Option<Vec<String>>,
    ) -> anyhow::Result<String> {
        let sessions = self.load_sessions(session_filter.as_deref())?;
        Ok(render_insights_markdown(&sessions))
    }

    fn load_sessions(&self, filter: Option<&[String]>) -> anyhow::Result<Vec<CodexSession>> {
        let Some(root) = &self.output_dir else {
            return Ok(vec![]);
        };
        if !root.exists() {
            return Ok(vec![]);
        }
        let mut files = Vec::new();
        collect_json_files(root, &mut files)?;
        files.sort();
        let mut out = Vec::new();
        for path in files {
            let raw = match std::fs::read_to_string(&path) {
                Ok(r) => r,
                Err(e) => {
                    // Present-but-unreadable session file: surface it (metadata
                    // only — never the contents) and skip rather than abort.
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping Codex session file that could not be read"
                    );
                    continue;
                }
            };
            let session = match parse_session(&raw) {
                Ok(s) => s,
                Err(e) => {
                    // Unparseable session file: surface it so a corrupt file is
                    // not silently dropped. `parse_session` errors carry only
                    // the serde position, never the file contents.
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping unparseable Codex session file"
                    );
                    continue;
                }
            };
            if let Some(filter) = filter
                && !filter.iter().any(|f| f == &session.session_id)
            {
                continue;
            }
            out.push(session);
        }
        Ok(out)
    }
}

fn collect_json_files(dir: &Path, acc: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    // A caller checks `root.exists()` before the top-level call, so reaching a
    // `read_dir` failure here means a present-but-unreadable directory. That is
    // a real failure and must propagate, not collapse to `Ok(())` with zero
    // files (which is indistinguishable from an empty directory).
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_json_files(&p, acc)?;
        } else if p.extension().and_then(|s| s.to_str()) == Some("json") {
            acc.push(p);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Issue #871 — `read_dir` failing must not become "success with zero files",
// and an unparseable session file must not be silently dropped.
//
// A MISSING output dir is the legitimately-empty case (silent Ok). An
// UNREADABLE dir must propagate an error. An unparseable session file must be
// warned-about-and-skipped (scan continues), without leaking file contents.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod issue_871_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    const FAKE_SECRET: &str = "ghp_FAKE_SECRET_do_not_log_0123456789";

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

    fn joined(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[test]
    fn collect_json_files_read_dir_error_propagates() {
        let dir = tempfile::tempdir().unwrap();
        // A file (not a directory) makes read_dir fail with ENOTDIR — an
        // unreadable-directory condition that fails for every user.
        let not_a_dir = dir.path().join("a-file");
        std::fs::write(&not_a_dir, b"x").unwrap();
        let mut acc = Vec::new();
        let result = collect_json_files(&not_a_dir, &mut acc);
        assert!(
            result.is_err(),
            "an unreadable directory must surface an error, not `Ok(())` with zero files"
        );
    }

    #[test]
    fn missing_output_dir_is_silent_empty() {
        let dir = tempfile::tempdir().unwrap();
        let builder = CodexTranscriptsBuilder::new(Some(dir.path().join("nope")));
        let (out, logs) = capture(|| builder.build_comprehensive_codex(None).unwrap());
        assert!(out.is_empty());
        assert!(
            logs.is_empty(),
            "an absent output dir is the normal empty case and must be silent; got: {logs:?}"
        );
    }

    #[test]
    fn unparseable_session_is_warned_and_skipped_without_leak() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("good.json"),
            r#"{"session_id":"sess-good","messages":[{"role":"assistant","content":"hello"}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("bad.json"),
            format!("{{ not valid json {FAKE_SECRET}"),
        )
        .unwrap();

        let builder = CodexTranscriptsBuilder::new(Some(dir.path().to_path_buf()));
        let (out, logs) = capture(|| builder.build_comprehensive_codex(None).unwrap());

        assert!(
            out.contains("sess-good"),
            "a valid session must still be processed despite a bad neighbour (skip, not abort)"
        );
        assert!(
            has_level(&logs, "WARN"),
            "an unparseable session file must be surfaced at warn!, not silently dropped; got: {logs:?}"
        );
        assert!(
            joined(&logs).contains("bad.json"),
            "the diagnostic must name the offending file; got: {logs:?}"
        );
        assert!(
            !joined(&logs).contains(FAKE_SECRET),
            "diagnostics must be metadata-only and must not leak file contents; got: {logs:?}"
        );
    }
}
