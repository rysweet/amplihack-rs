//! Export-on-compact integration.
//!
//! Native Rust port of `export_on_compact_integration.py`. Persists session
//! exports to disk so they can be enumerated and restored later.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSession {
    pub session_id: String,
    pub exported_path: String,
    pub trigger: String,
    pub exported_at: i64,
}

pub struct ExportOnCompactIntegration {
    root: PathBuf,
}

impl ExportOnCompactIntegration {
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    fn export_dir(&self) -> PathBuf {
        self.root.join("export-on-compact")
    }

    fn manifest_for(&self, session_id: &str) -> PathBuf {
        self.export_dir().join(format!("{session_id}.json"))
    }

    /// Process a single compact-trigger payload. The input must contain
    /// `session_id`, `transcript_path` and `trigger` fields.
    pub fn process(&self, input: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let obj = input
            .as_object()
            .ok_or_else(|| anyhow!("input must be a JSON object"))?;
        let session_id = obj
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing session_id"))?;
        let transcript_path = obj
            .get("transcript_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing transcript_path"))?;
        let trigger = obj
            .get("trigger")
            .and_then(|v| v.as_str())
            .unwrap_or("compact")
            .to_string();

        let transcript = Path::new(transcript_path);
        if !transcript.exists() {
            return Err(anyhow!("transcript not found at {transcript_path}"));
        }

        std::fs::create_dir_all(self.export_dir()).context("create export dir")?;
        let exported_path = self
            .export_dir()
            .join(format!("{session_id}.transcript.json"));
        std::fs::copy(transcript, &exported_path).context("copy transcript to export dir")?;

        let now = chrono::Utc::now().timestamp();
        let record = ExportedSession {
            session_id: session_id.to_string(),
            exported_path: exported_path.to_string_lossy().into_owned(),
            trigger: trigger.clone(),
            exported_at: now,
        };
        let manifest = self.manifest_for(session_id);
        std::fs::write(&manifest, serde_json::to_vec_pretty(&record)?)
            .context("write export manifest")?;

        Ok(serde_json::json!({
            "status": "success",
            "session_id": session_id,
            "exported_path": record.exported_path,
            "trigger": trigger,
            "manifest_path": manifest.to_string_lossy(),
            "exported_at": now,
        }))
    }

    /// Enumerate previously-exported sessions.
    pub fn list_available_sessions(&self) -> anyhow::Result<Vec<ExportedSession>> {
        let dir = self.export_dir();
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            // Skip `.transcript.json` payload files; only manifests count.
            let stem = match p.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => continue,
            };
            if stem.ends_with(".transcript") {
                continue;
            }
            let raw = match std::fs::read_to_string(&p) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Ok(rec) = serde_json::from_str::<ExportedSession>(&raw) {
                out.push(rec);
            }
        }
        out.sort_by(|a, b| a.exported_at.cmp(&b.exported_at));
        Ok(out)
    }

    /// Restore enhanced session data for a known session_id.
    pub fn restore_enhanced_session_data(
        &self,
        session_id: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let target = session_id.ok_or_else(|| anyhow!("session_id required"))?;
        let manifest = self.manifest_for(target);
        if !manifest.exists() {
            return Err(anyhow!("no exported session for id {target}"));
        }
        let raw = std::fs::read_to_string(&manifest)?;
        let rec: ExportedSession = serde_json::from_str(&raw).context("parse manifest")?;
        let transcript = std::fs::read_to_string(&rec.exported_path).unwrap_or_default();
        Ok(serde_json::json!({
            "session_id": rec.session_id,
            "exported_path": rec.exported_path,
            "trigger": rec.trigger,
            "exported_at": rec.exported_at,
            "transcript": transcript,
        }))
    }
}
