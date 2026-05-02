//! CodexTranscriptsBuilder — aggregates Codex session transcripts into a
//! comprehensive corpus, focused excerpts, and insight reports.
//!
//! Native Rust port of `codex_transcripts_builder.py`. The Python module
//! supports many output formats; the Rust port implements the subset
//! exercised by the test contract (`build_comprehensive_codex`,
//! `build_focused_codex`, `extract_learning_corpus`,
//! `generate_insights_report`).

use std::path::{Path, PathBuf};

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
                Err(_) => continue,
            };
            let session = match parse_session(&raw) {
                Ok(s) => s,
                Err(_) => continue,
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
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
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
