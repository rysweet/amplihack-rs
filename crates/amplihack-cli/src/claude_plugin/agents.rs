//! Selected wrapper agents and removal counts from the published wrapper.
//!
//! The wrapper digest authorises replacement of its contents. Its agent files
//! provide the previous selection without a separate record. All-mode wrappers
//! retain the whole mirror.

use super::*;
use amplihack_utils::list_selection::EntryId;

pub(super) fn inventory(root: &Path) -> Result<BTreeSet<String>> {
    if !root.is_dir() {
        return Ok(BTreeSet::new());
    }
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    let mut agents = BTreeSet::new();
    for (relative, kind) in entries {
        if !matches!(kind, TreeEntry::File) || relative.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let name = relative
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        let _: EntryId = format!("agent:{name}").parse()?;
        agents.insert(name);
    }
    Ok(agents)
}

pub(super) fn publish_selected(
    source: &Path,
    target: &Path,
    scratch: &mut Scratch,
    selection: &ListSelection,
) -> Result<bool> {
    let selected = inventory(source)?
        .into_iter()
        .filter(|agent| selection.includes_agent(agent))
        .collect::<Vec<_>>();
    // Hash the selected copy plan before creating any scratch files. An
    // unchanged launch must not rewrite the tree or create a transaction.
    let mut directories = BTreeSet::new();
    let mut plan = Vec::new();
    for agent in &selected {
        let path = PathBuf::from(format!("{agent}.md"));
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
        let relative = format!("{agent}.md");
        let destination = replacement.join(&relative);
        fs::create_dir_all(destination.parent().context("agent has no parent")?)?;
        copy_file(&source.join(relative), &destination)?;
    }
    rename_over(&replacement, target, scratch)?;
    Ok(true)
}
