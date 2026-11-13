//! Amplihack's work identity and launch admission; replay belongs to the runner.
use anyhow::{Context, Result};
use recipe_runner_rs::recovery::{admission::Admission, model::fingerprint, resources::git};
use std::{collections::BTreeMap, path::Path};

fn repository_name(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let name = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("git@github.com:"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;
    let parts: Vec<_> = name.split('/').collect();
    (parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        }))
    .then(|| name.to_ascii_lowercase())
}
fn number(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse::<u64>().ok())
        .flatten()
        .filter(|n| *n > 0)
}
/// Structural URL validation: no prose extraction, alternate host or conflicting issue.
fn issue(context: &BTreeMap<String, String>, repo: &Path) -> Option<u64> {
    let explicit = context.get("issue_number").filter(|v| !v.is_empty());
    let mut issue = explicit.and_then(|v| number(v));
    if explicit.is_some() && issue.is_none() {
        return None;
    }
    if let Some(url) = context.get("issue_url").filter(|v| !v.is_empty()) {
        let rest = url.strip_prefix("https://github.com/")?;
        let (repository, raw) = rest.split_once("/issues/")?;
        let remote = String::from_utf8(git(repo, &["remote", "get-url", "origin"]).ok()?).ok()?;
        if repository.to_ascii_lowercase() != repository_name(&remote)? {
            return None;
        }
        let from_url = number(raw)?;
        if issue.is_some_and(|n| n != from_url) {
            return None;
        }
        issue = Some(from_url);
    }
    issue
}
fn work_repository(
    working_dir: &Path,
    context: &BTreeMap<String, String>,
) -> Option<std::path::PathBuf> {
    let selected = context
        .get("repo_path")
        .filter(|v| !v.is_empty())
        .map_or(Path::new("."), |v| Path::new(v));
    let path = if selected.is_absolute() {
        selected.to_path_buf()
    } else {
        working_dir.join(selected)
    };
    path.canonicalize().ok()
}
/// Worktrees and symlink aliases share identity; independent clones do not.
pub(super) fn identity(
    recipe: &Path,
    recipe_name: &str,
    repo: &Path,
    context: &BTreeMap<String, String>,
) -> Option<String> {
    let repo = work_repository(repo, context)?;
    let issue = issue(context, &repo)?;
    let common = String::from_utf8(
        git(
            &repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .ok()?,
    )
    .ok()?;
    let common = Path::new(common.trim()).canonicalize().ok()?;
    let recipe = recipe.canonicalize().ok()?;
    fingerprint(&(recipe_name, recipe, common, issue)).ok()
}
/// Normalize explicit work metadata before evaluating inputs, so a URL and its
/// equivalent numeric issue select compatible context at relaunch.
pub(super) fn normalize(context: &mut BTreeMap<String, String>, repo: &Path) {
    if let Some(issue) = work_repository(repo, context).and_then(|repo| issue(context, &repo)) {
        context.insert("issue_number".into(), issue.to_string());
    }
}

/// Recover only the publication policy from the runner's admitted source.
/// Initial context values live in probes, rather than context deltas. Restore
/// this input before replay compares probes; credentials and merge authority
/// are never recovered here. Explicit caller context takes precedence.
pub(super) fn restore_issue_link(
    admission: &Admission,
    explicit: &BTreeMap<String, String>,
    context: &mut BTreeMap<String, String>,
) -> Result<()> {
    use recipe_runner_rs::recovery::{
        model::{ContextChange, Tree},
        private_fs::{Directory, parse},
    };
    let Some(source) = &admission.descriptor.source else {
        return Ok(());
    };
    if !context.contains_key("pr_issue_link") || explicit.contains_key("pr_issue_link") {
        return Ok(());
    }
    let directory = Directory::open(&source.directory, false)?;
    let tree: Tree = parse(&directory.read("tree.json")?)?;
    recipe_runner_rs::recovery::store::validate_tree(&tree)?;
    anyhow::ensure!(
        tree.run_id == source.run_id
            && tree.sequence == source.sequence
            && tree.identity == admission.descriptor.identity,
        "issue policy recovery source identity mismatch"
    );
    let mut value = serde_json::Value::String("auto".into());
    if let Some(saved) = tree
        .records
        .iter()
        .find_map(|r| r.probe.inputs.get("pr_issue_link"))
    {
        value = saved.clone();
    }
    for record in &tree.records {
        if let Some(delta) = record.delta.get("pr_issue_link") {
            value = match delta {
                ContextChange::Assign { value } => value.clone(),
                ContextChange::Remove => serde_json::Value::String("auto".into()),
            };
        }
    }
    let value = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("restored pr_issue_link must be a string"))?;
    crate::commands::workflow::issue_reference::validate_value(value)?;
    context.insert("pr_issue_link".into(), value.into());
    Ok(())
}
pub(super) fn admit(
    workspace: &crate::commands::cleanup::run_dir::RunWorkspace,
    recipe: &Path,
    name: &str,
    repo: &Path,
    context: &BTreeMap<String, String>,
    fresh: bool,
    settings: String,
) -> Result<Admission> {
    let crate::commands::cleanup::run_dir::RunWorkspace::Scoped { scope, .. } = workspace else {
        anyhow::bail!("durable recipe execution requires a private work root");
    };
    let identity = identity(recipe, name, repo, context);
    Admission::begin(
        &scope.work_root.path,
        &scope.run_dir,
        identity.as_deref(),
        fresh,
        settings,
    )
    .context("cannot durably admit recipe launch")
}
