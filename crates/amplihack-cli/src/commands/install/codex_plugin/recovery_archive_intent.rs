//! Intent and completion receipt share one durable marker; neither is unlinked.
use super::*;
use recovery_archive_storage::{Identity, capture};

pub(super) const NAME: &str = "recovery-archive.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    schema_version: u32,
    destination: String,
    identity: Identity,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    complete: bool,
}
impl Intent {
    pub fn new(destination: String, identity: Identity) -> Self {
        Self {
            schema_version: 1,
            destination,
            identity,
            complete: false,
        }
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        json_bytes(&serde_json::to_value(self)?)
    }
    fn validate(&self) -> Result<()> {
        let name = self
            .destination
            .strip_prefix("pending.json.stale-")
            .context("invalid Codex archive destination; intent retained")?;
        let mut parts = name.split('.');
        let stamp = parts.next().context("missing UTC archive stamp")?;
        let date = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")?;
        let suffix = parts.next();
        ensure!(
            self.schema_version == 1
                && date.format("%Y%m%dT%H%M%SZ").to_string() == stamp
                && parts.next().is_none()
                && suffix.is_none_or(|s| !s.is_empty()
                    && !s.starts_with('0')
                    && s.bytes().all(|b| b.is_ascii_digit())
                    && s.parse::<u64>().is_ok()),
            "unsupported or invalid Codex archive intent; record retained"
        );
        Ok(())
    }
    pub fn validate_receipt(&mut self, root: &Path) -> Result<()> {
        ensure!(
            self.complete,
            "Codex archival intent is still incomplete; record retained"
        );
        self.finish(root)?;
        Ok(())
    }
    pub fn finish(&mut self, root: &Path) -> Result<PathBuf> {
        self.validate()?;
        let source = root.join("pending.json");
        let destination = root.join(&self.destination);
        let intent_path = root.join(NAME);
        let intent =
            capture(&intent_path)?.context("Codex archive intent disappeared; record retained")?;
        ensure!(
            intent.bytes == self.bytes()?,
            "Codex archive intent changed; record retained"
        );
        // Repeat intent durability even after an interrupted publication.
        sync_path(&intent_path)?;
        sync_path(root)?;
        if self.complete {
            ensure!(
                capture(&destination)?.is_some_and(|c| c.identity == self.identity),
                "Codex archive evidence missing or changed; completion receipt retained"
            );
            sync_path(&destination)?;
            sync_path(root)?;
            ensure!(
                capture(&destination)?.is_some_and(|c| c.identity == self.identity)
                    && capture(&intent_path)?.is_some_and(|c| c.identity == intent.identity),
                "Codex archive evidence or completion receipt changed; record retained"
            );
            // A later transaction may have its own pending.json. A receipt
            // authorizes no change to that transaction or its targets.
            return Ok(destination);
        }
        let pending = capture(&source)?;
        let archived = capture(&destination)?;
        match (pending, archived) {
            (Some(pending), None) => {
                ensure!(
                    pending.identity == self.identity,
                    "Codex recovery source identity or bytes changed; intent retained"
                );
                recovery_archive_storage::rename(&source, &destination)?;
            }
            (None, Some(archived)) => {
                ensure!(
                    archived.identity == self.identity,
                    "Codex archive evidence changed; intent retained"
                );
            }
            _ => bail!(
                "Codex archive evidence missing or conflicting at {}; intent retained",
                destination.display()
            ),
        }
        ensure!(
            capture(&destination)?.is_some_and(|c| c.identity == self.identity)
                && capture(&source)?.is_none(),
            "Codex recovery evidence changed during archival; intent retained"
        );
        sync_path(&destination)?;
        sync_path(root)?;
        ensure!(
            capture(&intent_path)?.is_some_and(|c| c.identity == intent.identity),
            "Codex archive intent replaced; record retained"
        );
        // Atomic replacement keeps the old intent on pre-publication failure,
        // or the receipt on a final sync failure. No fallback write is needed.
        let original = serde_json::to_value(&*self)?;
        self.complete = true;
        atomic_json(&intent_path, &serde_json::to_value(&*self)?, Some(original))
            .context("Codex archive completion transition failed; intent or receipt retained")?;
        let receipt_bytes = self.bytes()?;
        ensure!(
            capture(&destination)?.is_some_and(|c| c.identity == self.identity)
                && capture(&source)?.is_none()
                && capture(&intent_path)?.is_some_and(|c| c.bytes == receipt_bytes),
            "Codex archive evidence or completion receipt changed; record retained"
        );
        Ok(destination)
    }
}
pub(super) fn resume(root: &Path) -> Result<Option<PathBuf>> {
    let Some(raw) = capture(&root.join(NAME))? else {
        return Ok(None);
    };
    let mut intent: Intent = serde_json::from_slice(&raw.bytes).with_context(|| {
        format!(
            "invalid Codex archive intent at {}; repair metadata manually; record retained",
            super::super::paths::sanitize_path(&root.join(NAME))
        )
    })?;
    let incomplete = !intent.complete;
    let destination = intent.finish(root)?;
    Ok(incomplete.then_some(destination))
}
