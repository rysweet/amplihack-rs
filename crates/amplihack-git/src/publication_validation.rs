//! Destination-aware publication: validate every outgoing commit and ref.
use crate::{
    isolated_history::History,
    task_scope::TaskScope,
    workspace_validation::{Workspace, bytes, run},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::Path;

/// One result shared by checkpointing, publication and recipe JSON consumers.
#[derive(Debug, Serialize)]
pub struct WorkflowResult {
    pub version: u32,
    pub committed: bool,
    pub reconciled: bool,
    pub commit_id: Option<String>,
    pub pushed: bool,
    pub push_reason: String,
    pub updates: Vec<PublicationUpdate>,
    pub errors: Vec<String>,
}
impl Default for WorkflowResult {
    fn default() -> Self {
        Self {
            version: 1,
            committed: false,
            reconciled: false,
            commit_id: None,
            pushed: false,
            push_reason: "not_attempted".to_owned(),
            updates: Vec::new(),
            errors: Vec::new(),
        }
    }
}
#[derive(Debug, Serialize)]
pub struct PublicationUpdate {
    destination: String,
    reference: String,
    commit_id: String,
    outcome: String,
}

/// Authority refusals retain status 78 through contextual error chains.
#[derive(Debug)]
pub struct PolicyRefusal(pub String);
impl std::fmt::Display for PolicyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PolicyRefusal {}

fn optional(root: &Path, args: &[&str]) -> Result<Option<String>> {
    let out = crate::command_in(root).args(args).output()?;
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    if !out.status.success() {
        bail!("cannot read publication configuration");
    }
    let value = String::from_utf8(out.stdout)?
        .strip_suffix('\n')
        .context("unterminated publication configuration")?
        .to_owned();
    if value.chars().any(char::is_control) {
        bail!("unsupported control character in publication configuration");
    }
    Ok((!value.is_empty()).then_some(value))
}
fn check_paths(history: &Path, scope: &TaskScope, args: &[&str]) -> Result<()> {
    for path in crate::isolated_history::bytes(history, args)?
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
    {
        scope.require(std::str::from_utf8(path).context("unsupported outgoing filename")?)?;
    }
    Ok(())
}

pub fn validate(history: &History, scope: &TaskScope, tips: &[String]) -> Result<()> {
    let cwd = history.directory.path();
    for tip in tips {
        for path in
            crate::isolated_history::bytes(cwd, &["ls-tree", "-r", "--name-only", "-z", tip])?
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
        {
            let path = std::str::from_utf8(path).context("unsupported outgoing filename")?;
            if amplihack_types::paths::owned_runtime::prohibited(path) {
                return Err(PolicyRefusal(format!(
                    "prohibited runtime path in outgoing tree {tip}: {path}"
                ))
                .into());
            }
        }
        let mut args = vec!["rev-list", tip.as_str(), "--not"];
        args.extend(history.advertised.iter().map(String::as_str));
        for commit in crate::isolated_history::run(cwd, &args)?.lines() {
            let parent_line =
                crate::isolated_history::run(cwd, &["rev-list", "--parents", "-n", "1", commit])?;
            let parts: Vec<_> = parent_line.split_whitespace().collect();
            let inspection = (|| -> Result<()> {
                match &parts[1..] {
                    [] => check_paths(
                        cwd,
                        scope,
                        &[
                            "diff-tree",
                            "--root",
                            "--no-commit-id",
                            "--no-renames",
                            "--name-only",
                            "-r",
                            "-z",
                            commit,
                        ],
                    )?,
                    [parent] => check_paths(
                        cwd,
                        scope,
                        &[
                            "diff",
                            "--no-ext-diff",
                            "--no-renames",
                            "--name-only",
                            "-z",
                            parent,
                            commit,
                        ],
                    )?,
                    [first, second] => {
                        // Imported remote parents are already excluded from the range.
                        // Inspect only changes to the automatic merge tree; checking a
                        // first-parent diff would incorrectly reject advertised imports.
                        let merged = crate::isolated_history::run(
                            cwd,
                            &["merge-tree", "--write-tree", first, second],
                        )
                        .context("cannot reconstruct automatic merge; publication refused")?;
                        let tree = merged
                            .lines()
                            .next()
                            .context("merge reconstruction returned no tree")?;
                        check_paths(
                            cwd,
                            scope,
                            &[
                                "diff",
                                "--no-ext-diff",
                                "--no-renames",
                                "--name-only",
                                "-z",
                                tree,
                                &format!("{commit}^{{tree}}"),
                            ],
                        )?;
                    }
                    _ => {
                        return Err(PolicyRefusal(format!(
                            "unsupported outgoing merge {commit}; publication refused"
                        ))
                        .into());
                    }
                }
                Ok(())
            })();
            inspection.with_context(|| format!("outgoing commit {commit}"))?;
        }
    }
    Ok(())
}

pub fn publish(root: &Path) -> Result<()> {
    publish_into(root, &mut WorkflowResult::default())
}

pub fn publish_into(root: &Path, result: &mut WorkflowResult) -> Result<()> {
    let workspace = Workspace::open(root)?;
    let root = workspace.root();
    workspace.require_workflow()?;
    let scope = TaskScope::from_recipe(&workspace)?;
    scope.check_workspace(&workspace)?;
    let branch = run(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = branch.trim();
    let remote = optional(
        root,
        &["config", "--get", &format!("branch.{branch}.pushRemote")],
    )?
    .or(optional(root, &["config", "--get", "remote.pushDefault"])?)
    .or(optional(
        root,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )?)
    .unwrap_or_else(|| "origin".to_owned());
    if remote == "." || remote.starts_with('-') {
        bail!("invalid publication remote");
    }
    // get-url is line delimited. Refuse embedded controls in its underlying
    // configuration instead of silently treating one URL as several remotes.
    crate::isolated_history::check_destination_config(root)?;
    let urls = run(root, &["remote", "get-url", "--push", "--all", &remote])?;
    let configured = crate::command_in(root)
        .args([
            "config",
            "--null",
            "--get-all",
            &format!("remote.{remote}.push"),
        ])
        .output()?;
    let specifications: Vec<String> = if configured.status.success() {
        configured
            .stdout
            .strip_suffix(&[0])
            .context("unterminated configured publication updates")?
            .split(|b| *b == 0)
            .map(|v| {
                String::from_utf8(v.to_vec()).context("unsupported publication refspec encoding")
            })
            .collect::<Result<_>>()?
    } else if configured.status.code() == Some(1) {
        // The inherited upstream describes the base, never publication authority.
        vec![format!("refs/heads/{branch}:refs/heads/{branch}")]
    } else {
        bail!("cannot read configured publication updates");
    };
    let mut updates = Vec::new();
    let mut source_refs = Vec::new();
    let mut target_refs = std::collections::BTreeSet::new();
    for spec in specifications {
        if spec.chars().any(char::is_control) {
            bail!("control character in publication refspec");
        }
        let (source, destination) = spec
            .split_once(':')
            .context("workflow requires explicit source:destination push refspecs")?;
        if !source.starts_with("refs/heads/")
            || !destination.starts_with("refs/heads/")
            || spec.contains(['*', '+', '^'])
        {
            bail!("unsupported publication refspec");
        }
        run(root, &["check-ref-format", source])?;
        run(root, &["check-ref-format", destination])?;
        if matches!(destination, "refs/heads/main" | "refs/heads/master")
            || optional(root, &["config", "--get", "init.defaultBranch"])?
                .is_some_and(|name| destination == format!("refs/heads/{name}"))
        {
            bail!("task publication refuses a protected/default branch destination: {destination}");
        }
        if !target_refs.insert(destination.to_owned()) {
            bail!("duplicate publication destination ref");
        }
        source_refs.push(source.to_owned());
        updates.push((
            run(
                root,
                &["rev-parse", "--verify", &format!("{source}^{{commit}}")],
            )?
            .trim()
            .to_owned(),
            destination.to_owned(),
        ));
    }
    if updates.is_empty() {
        bail!("publication requires at least one explicit update");
    }
    let mut tips: Vec<_> = updates.iter().map(|(oid, _)| oid.clone()).collect();
    let mut destinations = Vec::new();
    for url in urls.lines() {
        if url.is_empty() || url.starts_with('-') {
            bail!("invalid push destination");
        }
        let url = crate::isolated_history::resolve_destination(root, url)?;
        crate::isolated_history::require_literal_destination(root, &url)?;
        let history = History::obtain(root, &url, &tips)?;
        if history
            .default_branch
            .as_ref()
            .is_some_and(|reference| target_refs.contains(reference))
        {
            bail!("task publication refuses the destination default branch");
        }
        for (tip, reference) in &updates {
            validate(&history, &scope, std::slice::from_ref(tip)).map_err(|error| {
                let diagnostic = format!("destination {} ref {reference}: {error:#}", redact(&url));
                error.context(diagnostic)
            })?;
        }
        destinations.push((url, history));
    }
    if destinations.is_empty() {
        bail!("no actual push destination");
    }
    // Synchronisation imports only the frozen advertisement from the actual
    // destination. It never treats a local tracking ref or FETCH_HEAD as authority.
    if destinations.len() == 1
        && updates.len() == 1
        && source_refs[0] == format!("refs/heads/{branch}")
    {
        let history = &destinations[0].1;
        if let Some(remote_tip) = history.refs.get(&updates[0].1) {
            let ancestry = crate::isolated_history::command(history.directory.path())
                .args(["merge-base", "--is-ancestor", remote_tip, &tips[0]])
                .status()?;
            if ancestry.code() == Some(1) {
                if !bytes(
                    root,
                    &[
                        "diff",
                        "--cached",
                        "--ita-visible-in-index",
                        "--name-only",
                        "-z",
                    ],
                )?
                .is_empty()
                {
                    bail!(
                        "remote history requires synchronisation; protected staged work left untouched"
                    );
                }
                let isolated = history
                    .directory
                    .path()
                    .to_str()
                    .context("unsupported isolated history path")?;
                run(
                    root,
                    &[
                        "fetch",
                        "--atomic",
                        "--no-tags",
                        "--no-write-fetch-head",
                        isolated,
                        remote_tip,
                    ],
                )?;
                run(root, &["rebase", remote_tip])
                    .context("cannot synchronise task branch with advertised destination")?;
                let updated = run(root, &["rev-parse", "--verify", "HEAD^{commit}"])?
                    .trim()
                    .to_owned();
                crate::isolated_history::run(
                    history.directory.path(),
                    &[
                        "fetch",
                        "--atomic",
                        "--no-tags",
                        "--no-write-fetch-head",
                        root.to_str().context("unsupported checkout path")?,
                        &updated,
                    ],
                )?;
                tips[0] = updated.clone();
                updates[0].0 = updated;
                validate(history, &scope, &tips)?;
            } else if !ancestry.success() {
                bail!("cannot establish destination ancestry");
            }
        }
    }
    // All destinations and every explicit update have passed before any push.
    for (url, history) in &destinations {
        for (oid, reference) in &updates {
            result.updates.push(PublicationUpdate {
                destination: redact(url),
                reference: reference.clone(),
                commit_id: oid.clone(),
                outcome: if history.refs.get(reference) == Some(oid) {
                    "already_current"
                } else {
                    "not_attempted"
                }
                .to_owned(),
            });
        }
    }
    for (destination_number, (url, _)) in destinations.iter().enumerate() {
        if let Err(error) = crate::isolated_history::require_literal_destination(root, url) {
            result.push_reason = if destination_number > 0 {
                "partial"
            } else {
                "failed"
            }
            .to_owned();
            return Err(error);
        }
        let range = destination_number * updates.len()..(destination_number + 1) * updates.len();
        // Run normal push hooks even for an already-current destination.
        let mut command = crate::command_in(root);
        command.args([
            "-c",
            "push.followTags=false",
            "-c",
            "push.recurseSubmodules=no",
            "push",
            "--recurse-submodules=no",
            "--no-follow-tags",
            "--atomic",
            url,
        ]);
        for (oid, destination) in &updates {
            command.arg(format!("{oid}:{destination}"));
        }
        let output = command.output();
        let failure = match output {
            Ok(output) => {
                eprint!("{}", redact(&String::from_utf8_lossy(&output.stdout)));
                eprint!("{}", redact(&String::from_utf8_lossy(&output.stderr)));
                (!output.status.success())
                    .then(|| format!("publication transport failed: {}", output.status))
            }
            Err(error) => Some(format!("cannot start publication transport: {error}")),
        };
        if let Some(error) = failure {
            for update in &mut result.updates[range] {
                update.outcome = "failed".to_owned();
            }
            result.push_reason = if destination_number > 0 {
                "partial"
            } else {
                "failed"
            }
            .to_owned();
            bail!("{error}");
        }
        for update in &mut result.updates[range] {
            if update.outcome != "already_current" {
                update.outcome = "pushed".to_owned();
            }
        }
    }
    // Tracking changes only after every transport has succeeded.
    if updates.len() == 1 && source_refs[0] == format!("refs/heads/{branch}") {
        run(
            root,
            &["config", &format!("branch.{branch}.remote"), &remote],
        )?;
        run(
            root,
            &["config", &format!("branch.{branch}.merge"), &updates[0].1],
        )?;
    }
    result.pushed = true;
    result.push_reason = if result
        .updates
        .iter()
        .all(|update| update.outcome == "already_current")
    {
        "already_current"
    } else {
        "success"
    }
    .to_owned();
    Ok(())
}

/// Avoid exposing credentials in transport diagnostics.
pub(crate) fn redact(value: &str) -> String {
    value
        .split_inclusive(char::is_whitespace)
        .map(|word| {
            let mut result = word.to_owned();
            if let Some(scheme) = result.find("://") {
                let start = scheme + 3;
                let end = result[start..]
                    .find(['/', '\n', ' '])
                    .map_or(result.len(), |i| start + i);
                if let Some(at) = result[start..end].rfind('@') {
                    result.replace_range(start..start + at + 1, "<redacted>@");
                }
                if let Some(query) = result.find('?') {
                    result.truncate(query);
                    result.push_str("?<redacted>");
                }
            }
            result
        })
        .collect()
}
