//! Selected wrapper agents and their separate ownership record.
//!
//! The wrapper digest authorises replacement of its contents. The agent
//! record records individual files without changing the skill record that
//! older binaries must still read. All-mode wrappers retain the whole mirror.

use super::*;
use amplihack_utils::list_selection::EntryId;

const RECORD: &str = "claude-agents-manifest.json";

pub(super) fn record_path(staged: &Path) -> PathBuf {
    skill_ownership_manifest_path(staged).with_file_name(RECORD)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentOwnership {
    version: u32,
    agents: Vec<OwnedDestination>,
}

fn inventory(root: &Path) -> Result<Vec<OwnedDestination>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    let mut agents = Vec::new();
    for (relative, kind) in entries {
        if !matches!(kind, TreeEntry::File) || relative.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let name = relative
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        let _: EntryId = format!("agent:{name}").parse()?;
        agents.push(OwnedDestination {
            name,
            content_sha256: format!("{:x}", Sha256::digest(fs::read(root.join(relative))?)),
        });
    }
    agents.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(agents)
}

/// Called only after the complete wrapper has passed its ownership check.
/// A pre-feature wrapper has no separate record, so inspect its verified tree.
pub(super) fn previous_inventory(staged: &Path, plugin: &Path) -> Result<BTreeSet<String>> {
    let path = record_path(staged);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot inspect Claude agent ownership record"),
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Claude agent ownership record is not a regular file"
            );
            let record: AgentOwnership = serde_json::from_slice(&fs::read(&path)?)
                .context("cannot read Claude agent ownership record")?;
            anyhow::ensure!(
                record.version == 1,
                "unsupported Claude agent ownership version"
            );
            let mut seen = BTreeSet::new();
            for agent in record.agents {
                let _: EntryId = format!("agent:{}", agent.name).parse()?;
                validate_sha256(&agent.content_sha256)?;
                anyhow::ensure!(
                    seen.insert(agent.name),
                    "duplicate Claude agent ownership entry"
                );
            }
        }
    }
    // Actual listed files determine migration counts. The record can be from
    // before a successfully published wrapper whose record write was interrupted.
    Ok(inventory(&plugin.join("agents"))?
        .into_iter()
        .map(|agent| agent.name)
        .collect())
}

pub(super) fn record_inventory(
    staged: &Path,
    plugin: &Path,
    previous: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let record = AgentOwnership {
        version: 1,
        agents: inventory(&plugin.join("agents"))?,
    };
    let current = record
        .agents
        .iter()
        .map(|agent| agent.name.clone())
        .collect::<BTreeSet<_>>();
    let path = record_path(staged);
    let bytes = serde_json::to_vec_pretty(&record)?;
    if !file_has_contents(&path, &bytes)? {
        let temporary =
            tempfile::NamedTempFile::new_in(path.parent().context("agent record has no parent")?)?;
        fs::write(temporary.path(), &bytes)?;
        temporary
            .persist(&path)
            .context("cannot publish Claude agent ownership record")?;
    }
    Ok(previous.difference(&current).cloned().collect())
}

pub(super) fn remove_record(staged: &Path) -> Result<()> {
    match fs::remove_file(record_path(staged)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("cannot remove Claude agent ownership record"),
    }
}

pub(super) fn publish_selected(
    source: &Path,
    target: &Path,
    scratch: &mut Scratch,
    selection: &ListSelection,
) -> Result<bool> {
    let selected = inventory(source)?
        .into_iter()
        .filter(|agent| selection.includes_agent(&agent.name))
        .collect::<Vec<_>>();
    // Hash the selected copy plan before creating any scratch files. An
    // unchanged launch must not rewrite the tree or create a transaction.
    let mut directories = BTreeSet::new();
    let mut plan = Vec::new();
    for agent in &selected {
        let path = PathBuf::from(format!("{}.md", agent.name));
        let mut parent = path.parent();
        while let Some(directory) = parent.filter(|path| !path.as_os_str().is_empty()) {
            directories.insert(directory.to_path_buf());
            parent = directory.parent();
        }
        plan.push((path, TreeEntry::File));
    }
    plan.extend(
        directories
            .into_iter()
            .map(|path| (path, TreeEntry::Directory)),
    );
    plan.sort_by(|left, right| left.0.cmp(&right.0));
    if is_real_directory(target)?
        && hash_directory_tree(target)? == hash_tree_entries(source, plan)?
    {
        return Ok(false);
    }
    let replacement = scratch.path("selected-agents")?;
    fs::create_dir_all(&replacement)?;
    for agent in &selected {
        let relative = format!("{}.md", agent.name);
        let destination = replacement.join(&relative);
        fs::create_dir_all(destination.parent().context("agent has no parent")?)?;
        copy_file(&source.join(relative), &destination)?;
    }
    rename_over(&replacement, target, scratch)?;
    Ok(true)
}
