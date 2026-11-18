//! Closed command grammar for the local hosting fixture, not a GitHub emulator.
use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;

pub const IDENTITY_QUERY: &str = "query($owner:String!,$name:String!){viewer{login} repository(owner:$owner,name:$name){nameWithOwner viewerPermission}}";
pub const THREADS_QUERY: &str = "query($owner:String!,$name:String!,$pr:Int!,$endCursor:String){repository(owner:$owner,name:$name){pullRequest(number:$pr){reviewThreads(first:100,after:$endCursor){pageInfo{hasNextPage endCursor}nodes{isResolved}}}}}";

pub fn validate(args: &[&str], repository: &str) -> Result<Vec<String>> {
    let command = args.first().copied().context("hosting command absent")?;
    let operation = args.get(1).copied().context("hosting operation absent")?;
    let (start, values, switches, max_positions): (usize, &[&str], &[&str], usize) =
        match (command, operation) {
            ("api", _) => (
                1,
                &["--method", "--hostname", "--jq", "-f", "-F"],
                &["--paginate"],
                1,
            ),
            ("auth", "status") => (2, &["--hostname"], &[], 0),
            ("repo", "view") => (2, &["--repo", "--json", "--jq"], &[], 1),
            ("issue", "view") => (2, &["--repo", "--json", "--jq"], &[], 1),
            ("pr", "create") => (
                2,
                &[
                    "--repo",
                    "--head",
                    "--base",
                    "--title",
                    "--body",
                    "--body-file",
                ],
                &["--draft"],
                0,
            ),
            ("pr", "list") => (
                2,
                &[
                    "--repo", "--head", "--base", "--state", "--limit", "--json", "--jq",
                ],
                &[],
                0,
            ),
            ("pr", "view") => (2, &["--repo", "--json", "--jq"], &["--comments"], 1),
            ("pr", "checks") => (2, &["--repo", "--json", "--jq"], &["--required"], 1),
            ("pr", "merge") => (
                2,
                &["--repo", "--match-head-commit"],
                &["--squash", "--merge", "--rebase", "--delete-branch"],
                1,
            ),
            ("pr", "comment" | "edit") => (2, &["--repo", "--body", "--body-file"], &[], 1),
            ("pr", "review") => (
                2,
                &["--repo", "--body", "--body-file"],
                &["--approve", "--comment"],
                1,
            ),
            ("pr", "ready") => (2, &["--repo"], &["--undo"], 1),
            _ => bail!("unsupported hosting operation: {args:?}"),
        };
    let mut options = BTreeMap::new();
    let mut fields = BTreeMap::new();
    let mut positions = Vec::new();
    let mut normalized: Vec<String> = args[..start].iter().map(|s| (*s).into()).collect();
    let mut index = start;
    while index < args.len() {
        let arg = args[index];
        if !arg.starts_with('-') {
            positions.push(arg);
            normalized.push(arg.into());
            index += 1;
            continue;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg, None), |(key, value)| (key, Some(value)));
        let flag = match flag {
            "-R" => "--repo",
            "-q" => "--jq",
            "-X" => "--method",
            other => other,
        };
        ensure!(
            values.contains(&flag) || switches.contains(&flag),
            "unsupported hosting flag {flag}"
        );
        let value = if switches.contains(&flag) {
            ensure!(inline.is_none(), "switch has unexpected value");
            ""
        } else {
            let value = if let Some(value) = inline {
                value
            } else {
                index += 1;
                args.get(index)
                    .copied()
                    .context("hosting option value absent")?
            };
            ensure!(
                !value.is_empty() && !value.starts_with("--"),
                "invalid hosting option value"
            );
            value
        };
        if ["-f", "-F"].contains(&flag) {
            let (key, value) = value
                .split_once('=')
                .context("GraphQL field requires key=value")?;
            ensure!(
                fields.insert(key, value).is_none(),
                "duplicate GraphQL field"
            );
        } else {
            ensure!(
                options.insert(flag, value).is_none(),
                "duplicate hosting option"
            );
        }
        normalized.push(flag.into());
        if !switches.contains(&flag) {
            normalized.push(value.into());
        }
        index += 1;
    }
    ensure!(
        positions.len() <= max_positions,
        "unexpected trailing hosting arguments"
    );
    for (key, value) in &options {
        match *key {
            "--repo" => ensure!(*value == repository, "wrong hosting repository"),
            "--hostname" => ensure!(*value == "github.com", "unsupported hosting hostname"),
            "--base" => ensure!(*value == "main", "unsupported hosting base"),
            "--state" => ensure!(
                ["open", "closed", "merged", "all"].contains(value),
                "unsupported PR state"
            ),
            "--limit" => ensure!(
                value.parse::<u64>().is_ok_and(|v| v > 0),
                "invalid PR limit"
            ),
            _ => {}
        }
    }
    ensure!(
        !(options.contains_key("--body") && options.contains_key("--body-file")),
        "ambiguous body source"
    );
    if command == "issue" {
        ensure!(positions == ["7"], "unsupported issue identity");
    }
    if command == "repo" {
        ensure!(
            positions.is_empty() || positions == [repository],
            "unsupported repository identity"
        );
    }
    if command == "api" {
        ensure!(positions.len() == 1, "API endpoint absent");
        let endpoint = positions[0];
        if endpoint == "graphql" {
            ensure!(
                options
                    .get("--method")
                    .is_none_or(|method| *method == "POST"),
                "unsupported GraphQL method"
            );
            let query = fields.get("query").context("GraphQL query absent")?;
            let compact: String = query.chars().filter(|c| !c.is_whitespace()).collect();
            let identity: String = IDENTITY_QUERY
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            ensure!(
                compact == identity || compact == THREADS_QUERY,
                "unsupported GraphQL operation"
            );
            ensure!(
                fields
                    .get("owner")
                    .is_some_and(|v| ["fixture", "{owner}"].contains(v))
                    && fields
                        .get("name")
                        .is_some_and(|v| ["project", "{repo}"].contains(v)),
                "unsupported GraphQL repository"
            );
            if compact == identity {
                ensure!(fields.len() == 3, "unexpected identity query fields");
            } else {
                ensure!(
                    fields.len() == 4
                        && fields
                            .get("pr")
                            .is_some_and(|v| v.parse::<u64>().is_ok_and(|v| v > 0)),
                    "unsupported thread query fields"
                );
            }
        } else {
            ensure!(fields.is_empty(), "REST fields are unsupported");
            ensure!(
                options
                    .get("--method")
                    .is_none_or(|method| *method == "GET"),
                "unsupported REST method"
            );
            validate_endpoint(endpoint, repository)?;
        }
    }
    Ok(normalized)
}

fn validate_endpoint(endpoint: &str, repository: &str) -> Result<()> {
    let endpoint = endpoint.replace("repos/{owner}/{repo}/", &format!("repos/{repository}/"));
    let (path, query) = endpoint
        .split_once('?')
        .map_or((endpoint.as_str(), None), |(p, q)| (p, Some(q)));
    let valid_sha = |sha: &str| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    let Some(relative) = path.strip_prefix(&format!("repos/{repository}/")) else {
        ensure!(
            path == "user" && query.is_none(),
            "unsupported REST endpoint"
        );
        return Ok(());
    };
    let parts: Vec<_> = relative.split('/').collect();
    let supported = match parts.as_slice() {
        ["branches", "main"]
        | ["branches", "main", "protection"]
        | [
            "branches",
            "main",
            "protection",
            "required_pull_request_reviews",
        ]
        | ["rules", "branches", "main"]
        | ["actions", "workflows"]
        | ["actions", "runs"]
        | ["contents", ".github", "workflows"] => true,
        ["commits", sha, "check-runs" | "status"] => valid_sha(sha),
        ["issues", number, "comments"] | ["pulls", number, "reviews"] => {
            number.parse::<u64>().is_ok_and(|v| v > 0)
        }
        _ => false,
    };
    ensure!(supported, "unsupported REST endpoint {endpoint}");
    if let Some(query) = query {
        ensure!(
            (relative.ends_with("/check-runs") && query == "filter=all")
                || (relative == "contents/.github/workflows"
                    && query.strip_prefix("ref=").is_some_and(valid_sha)),
            "unsupported REST query parameters"
        );
    }
    Ok(())
}
