//! A durable intent keeps recovery protection across rename or sync failure.
use super::*;
use recovery_archive_storage::{Identity, capture, publish};

pub(super) const NAME: &str = "recovery-archive.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    schema_version: u32,
    destination: String,
    identity: Identity,
}
impl Intent {
    pub fn new(destination: String, identity: Identity) -> Self {
        Self {
            schema_version: 1,
            destination,
            identity,
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
    pub fn finish(&self, root: &Path) -> Result<PathBuf> {
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
        path_scope::check()?;
        fs::remove_file(&intent_path)?;
        if let Err(error) = sync_path(root) {
            // Preserve the guard on a reported final durability failure too.
            if let Err(restore) = publish(&intent_path, &self.bytes()?) {
                bail!(
                    "{error:#}; restoring archival protection {} failed: {restore:#}",
                    intent_path.display()
                );
            }
            return Err(error);
        }
        Ok(destination)
    }
}
pub(super) fn resume(root: &Path) -> Result<Option<PathBuf>> {
    let Some(raw) = capture(&root.join(NAME))? else {
        return Ok(None);
    };
    let intent: Intent = serde_json::from_slice(&raw.bytes).with_context(|| {
        format!(
            "invalid Codex archive intent at {}; repair metadata manually; record retained",
            super::super::paths::sanitize_path(&root.join(NAME))
        )
    })?;
    intent.finish(root).map(Some)
}
