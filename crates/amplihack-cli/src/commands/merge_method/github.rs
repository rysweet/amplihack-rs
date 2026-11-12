//! What GitHub says about the pull request's repository (#1888): the
//! repository named by the pull request URL, its merge settings, and the
//! active rulesets on the base branch. Every read is `gh api --hostname HOST`
//! with a path built from validated parts only. Contract:
//! docs/reference/auto-drive-to-merge.md, "Which repository's permissions are
//! read".
use super::{MergeMethod, Methods};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::process::Command;

/// The repository settings: `repos/OWNER/REPO`'s three `allow_*` fields.
#[derive(Debug, serde::Deserialize)]
struct Settings {
    allow_merge_commit: bool,
    allow_squash_merge: bool,
    allow_rebase_merge: bool,
}
impl From<Settings> for Methods {
    fn from(s: Settings) -> Self {
        Self {
            merge: s.allow_merge_commit,
            squash: s.allow_squash_merge,
            rebase: s.allow_rebase_merge,
        }
    }
}

/// What the active rulesets on the base branch allow: the intersection of
/// the `allowed_merge_methods` of every `pull_request` rule that lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BranchRules {
    /// How many `pull_request` rules list `allowed_merge_methods`.
    pub(super) restricting: usize,
    pub(super) allowed: Methods,
}

#[derive(Debug, thiserror::Error)]
#[error("gh api refused to read {0} (terminal policy exit 79)")]
pub(crate) struct PolicyRefusal(String);

/// The pull request's repository, from its URL `https://HOST/OWNER/REPO/pull/N`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Repository {
    host: String,
    pub(super) owner: String,
    pub(super) name: String,
}

pub(super) fn parse_pr_url(url: &str) -> Result<Repository> {
    const BAD: &str = "the pull request URL is not https://HOST/OWNER/REPO/pull/NUMBER";
    let rest = url.strip_prefix("https://").context(BAD)?;
    let parts: Vec<&str> = rest.split('/').collect();
    let [host, owner, name, "pull", number] = parts.as_slice() else {
        bail!(BAD);
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['.', '-'])
        && host
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-');
    // GitHub's own rules: an owner is 1-39 letters, digits and inner hyphens;
    // a repository name is 1-100 letters, digits, `.`, `_` and `-`.
    let owner_ok = !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && owner
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-');
    let name_ok = !name.is_empty()
        && name.len() <= 100
        && *name != "."
        && *name != ".."
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c));
    let number_ok = !number.is_empty()
        && !number.starts_with('0')
        && number.bytes().all(|c| c.is_ascii_digit());
    anyhow::ensure!(host_ok && owner_ok && name_ok && number_ok, BAD);
    Ok(Repository {
        host: host.to_string(),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// A base branch name that is safe as the last part of an API path: the
/// alphabet `autodrive_base_sha` accepts, no leading `-`, and no empty,
/// `.`-led or `.lock` component, so `.` and `..` never reach the URL.
pub(super) fn is_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c))
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// `gh api --hostname HOST <args>`. Exit 79 is a terminal policy refusal.
fn gh_api(repository: &Repository, args: &[&str], what: &str) -> Result<Vec<u8>> {
    let output = Command::new("gh")
        .args(["api", "--hostname", &repository.host])
        .args(args)
        .output()
        .with_context(|| format!("cannot execute gh api for {what}"))?;
    if output.status.code() == Some(79) {
        return Err(PolicyRefusal(what.to_string()).into());
    }
    anyhow::ensure!(
        output.status.success(),
        "gh api could not read {what} (status {})",
        output.status
    );
    Ok(output.stdout)
}

pub(super) fn repository_settings(repository: &Repository) -> Result<Methods> {
    let path = format!("repos/{}/{}", repository.owner, repository.name);
    let body = gh_api(repository, &[&path], "repository merge permissions")?;
    let response: serde_json::Value =
        serde_json::from_slice(&body).context("invalid repository merge permissions JSON")?;
    anyhow::ensure!(
        response.is_object(),
        "repository merge permissions must be an object"
    );
    let settings: Settings = serde_json::from_value(response).context(
        "invalid repository merge permissions: all three allow_* fields must be booleans",
    )?;
    Ok(settings.into())
}

/// Every active rule on `base_ref`, from `rules/branches/<base_ref>`, read
/// with `--paginate` so a rule on the second page counts.
pub(super) fn branch_rules(repository: &Repository, base_ref: &str) -> Result<BranchRules> {
    let path = format!(
        "repos/{}/{}/rules/branches/{base_ref}",
        repository.owner, repository.name
    );
    let what = format!("the rulesets on branch {base_ref}");
    let body = gh_api(repository, &["--paginate", &path], &what)?;
    parse_branch_rules(&body).map_err(|problem| {
        anyhow::anyhow!(
            "invalid answer for {what}: {problem}; an unreadable rule is not an absent rule"
        )
    })
}

/// The answer to `gh api --paginate rules/branches/<branch>`: one JSON array
/// per page, written one after another, or one array. Each element is a rule
/// object with a string `type`. A `pull_request` rule has a `parameters`
/// object; its `allowed_merge_methods`, when present and not null, is an
/// array of strings. Absent or null, the rule does not limit the method,
/// as for a ruleset created before GitHub added the parameter. A string other
/// than merge, squash or rebase is ignored: it can only name a method this
/// gate never uses, so ignoring it never allows a method the rule does not
/// list. Errors are fixed phrases, so no response text reaches the evidence.
fn parse_branch_rules(body: &[u8]) -> std::result::Result<BranchRules, &'static str> {
    let mut pages = 0usize;
    let mut rules = BranchRules {
        restricting: 0,
        allowed: Methods::ALL,
    };
    for page in serde_json::Deserializer::from_slice(body).into_iter::<Value>() {
        let page = page.map_err(|_| "not JSON")?;
        let items = page.as_array().ok_or("a page is not a JSON array")?;
        pages += 1;
        for rule in items {
            let rule = rule.as_object().ok_or("a rule is not an object")?;
            let kind = rule
                .get("type")
                .and_then(Value::as_str)
                .ok_or("a rule has no string type")?;
            if kind != "pull_request" {
                continue;
            }
            let parameters = rule
                .get("parameters")
                .and_then(Value::as_object)
                .ok_or("a pull_request rule has no parameters object")?;
            let listed = match parameters.get("allowed_merge_methods") {
                None | Some(Value::Null) => continue,
                Some(Value::Array(listed)) => listed,
                Some(_) => return Err("allowed_merge_methods is not an array"),
            };
            let mut allowed = Methods::NONE;
            for value in listed {
                let value = value
                    .as_str()
                    .ok_or("allowed_merge_methods holds a value that is not a string")?;
                if let Some(method) = MergeMethod::parse(value) {
                    allowed = allowed.with(method);
                }
            }
            rules.restricting += 1;
            rules.allowed = rules.allowed.and(allowed);
        }
    }
    if pages == 0 {
        return Err("no JSON array");
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn methods(merge: bool, squash: bool, rebase: bool) -> Methods {
        Methods {
            merge,
            squash,
            rebase,
        }
    }

    #[test]
    fn branch_rules_are_intersected_across_rules_and_pages() {
        let rule = |m: &str| {
            format!(r#"{{"type":"pull_request","parameters":{{"allowed_merge_methods":{m}}}}}"#)
        };
        let other = r#"{"type":"required_linear_history"}"#;
        let plain = r#"{"type":"pull_request","parameters":{"required_approving_review_count":1}}"#;
        let null = r#"{"type":"pull_request","parameters":{"allowed_merge_methods":null}}"#;
        for (body, restricting, want) in [
            ("[]".to_string(), 0, Methods::ALL),
            ("[]\n[]".to_string(), 0, Methods::ALL),
            (format!("[{other},{plain},{null}]"), 0, Methods::ALL),
            (
                format!(r#"[{}]"#, rule(r#"["merge"]"#)),
                1,
                methods(true, false, false),
            ),
            (
                format!(
                    r#"[{},{other}][{}]"#,
                    rule(r#"["merge","rebase"]"#),
                    rule(r#"["rebase","squash"]"#)
                ),
                2,
                methods(false, false, true),
            ),
            (format!("[{}]", rule("[]")), 1, Methods::NONE),
            (
                format!(r#"[{}]"#, rule(r#"["merge","fast-forward","MERGE"]"#)),
                1,
                methods(true, false, false),
            ),
        ] {
            let got = parse_branch_rules(body.as_bytes()).unwrap();
            assert_eq!(
                got,
                BranchRules {
                    restricting,
                    allowed: want
                },
                "{body}"
            );
        }
        for bad in [
            "",
            " ",
            "not json",
            "null",
            "{}",
            "[1]",
            "[null]",
            r#"[{"type":1}]"#,
            r#"[{"parameters":{}}]"#,
            r#"[{"type":"pull_request"}]"#,
            r#"[{"type":"pull_request","parameters":[]}]"#,
            r#"[{"type":"pull_request","parameters":{"allowed_merge_methods":"merge"}}]"#,
            r#"[{"type":"pull_request","parameters":{"allowed_merge_methods":[1]}}]"#,
            "[] garbage",
            "[]{}",
        ] {
            assert!(
                parse_branch_rules(bad.as_bytes()).is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn pr_url_names_the_repository() {
        assert_eq!(
            parse_pr_url("https://github.com/Owner-A/Repo_B.c/pull/5038").unwrap(),
            Repository {
                host: "github.com".into(),
                owner: "Owner-A".into(),
                name: "Repo_B.c".into()
            }
        );
        assert!(parse_pr_url("https://ghe.example.com/my-org/x.y_z/pull/1").is_ok());
        for bad in [
            "",
            "u",
            "http://github.com/o/r/pull/1",
            "https://github.com/o/r/pull/",
            "https://github.com/o/r/pull/01",
            "https://github.com/o/r/issues/1",
            "https://github.com/o/r/pull/1/files",
            "https://github.com/my_org/x/pull/1",
            "https://github.com/-o/r/pull/1",
            "https://github.com/o/../pull/1",
            "https://github.com/o/r;x/pull/1",
            "https://-github.com/o/r/pull/1",
            "https://github.com:443/o/r/pull/1",
        ] {
            assert!(parse_pr_url(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn base_branch_names_cannot_leave_the_rules_path() {
        for good in ["main", "release/1.0", "feat/issue-1888_x", "v2.x"] {
            assert!(is_branch(good), "refused {good:?}");
        }
        for bad in [
            "",
            "-main",
            "/main",
            "main/",
            "a//b",
            "a/../b",
            "..",
            ".",
            "a/./b",
            "a/.hidden",
            ".main",
            "main.",
            "main.lock",
            "a b",
            "a?b",
            "a#b",
            "a%2fb",
            "a\nb",
        ] {
            assert!(!is_branch(bad), "accepted {bad:?}");
        }
    }
}
