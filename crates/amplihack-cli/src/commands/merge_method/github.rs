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

/// The three kinds of active ruleset rule on the base branch that limit the
/// merge method. Every other rule type leaves the method alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BranchRules {
    /// How many `pull_request` rules list `allowed_merge_methods`.
    pub(super) method_rules: usize,
    /// The intersection of those lists; every method when there are none.
    pub(super) listed: Methods,
    /// How many `merge_queue` rules are active. GitHub merges a queued pull
    /// request with the queue's own `merge_method`, whatever method the merge
    /// request names, so each such rule allows that one method only.
    pub(super) queue_rules: usize,
    /// The intersection of those rules' methods; every method when there are
    /// none.
    pub(super) queued: Methods,
    /// A `required_linear_history` rule is active. GitHub then accepts a pull
    /// request only by squash or rebase, so it refuses `merge`.
    pub(super) linear_history: bool,
}

/// The phrase that names a `required_linear_history` rule, in the rationale
/// and in refusals.
const LINEAR_HISTORY: &str = "a required_linear_history rule forbids merge";

impl BranchRules {
    /// No rule limits the method.
    pub(super) const UNLIMITED: Self = Self {
        method_rules: 0,
        listed: Methods::ALL,
        queue_rules: 0,
        queued: Methods::ALL,
        linear_history: false,
    };

    /// What these rules allow: the methods every `pull_request` rule lists
    /// and every merge queue uses, less `merge` under a linear-history rule.
    pub(super) fn allowed(self) -> Methods {
        let allowed = self.listed.and(self.queued);
        if self.linear_history {
            allowed.without(MergeMethod::Merge)
        } else {
            allowed
        }
    }

    /// Each active limit, `; `-separated, for a refusal; empty when none is.
    pub(super) fn limits(self) -> String {
        let mut parts = Vec::new();
        if self.method_rules > 0 {
            parts.push(format!(
                "pull_request allowed_merge_methods allow {}",
                self.listed.names()
            ));
        }
        if self.queue_rules > 0 {
            parts.push(format!(
                "merge_queue merge_method allows {}",
                self.queued.names()
            ));
        }
        if self.linear_history {
            parts.push(LINEAR_HISTORY.to_string());
        }
        parts.join("; ")
    }

    /// The rationale's account of the rules on `base_ref`.
    pub(super) fn describe(self, base_ref: &str) -> String {
        let mut parts = Vec::new();
        if self.method_rules > 0 {
            parts.push(format!(
                "{} ruleset pull_request rule(s) on branch {base_ref} allow {}",
                self.method_rules,
                self.listed.names()
            ));
        }
        if self.queue_rules > 0 {
            parts.push(format!(
                "{} ruleset merge_queue rule(s) on branch {base_ref} allow {}",
                self.queue_rules,
                self.queued.names()
            ));
        }
        if self.linear_history {
            parts.push(format!(
                "a required_linear_history rule on branch {base_ref} forbids merge"
            ));
        }
        if parts.is_empty() {
            format!("no ruleset on branch {base_ref} limits the merge method")
        } else {
            parts.join("; ")
        }
    }
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
/// object with a string `type`. Three types limit the merge method:
///
/// - `required_linear_history`: GitHub accepts a pull request into the
///   branch only by squash or rebase, so `merge` is not allowed. The rule
///   has no parameters to read.
/// - `merge_queue`: `gh pr merge` adds the pull request to the queue, and the
///   queue merges it with the rule's `merge_method`, whatever method the
///   request names. So the rule allows that one method. Its `parameters`
///   object must hold `merge_method` as exactly `MERGE`, `SQUASH` or
///   `REBASE`, the values GitHub's schema defines; anything else is an
///   unreadable rule, since guessing the queue's method could record a method
///   that does not land.
/// - `pull_request`: it has a `parameters` object; its
///   `allowed_merge_methods`, when present and not null, is an array of
///   strings. Absent or null, the rule does not limit the method, as for a
///   ruleset created before GitHub added the parameter. A string other than
///   merge, squash or rebase is ignored: it can only name a method auto-drive
///   never uses, so ignoring it never allows a method the rule does not list.
///
/// Errors are fixed phrases, so no response text reaches the evidence.
fn parse_branch_rules(body: &[u8]) -> std::result::Result<BranchRules, &'static str> {
    let mut pages = 0usize;
    let mut rules = BranchRules::UNLIMITED;
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
            match kind {
                "required_linear_history" => {
                    rules.linear_history = true;
                    continue;
                }
                "merge_queue" => {
                    let method = queue_method(rule)?;
                    rules.queue_rules += 1;
                    rules.queued = rules.queued.and(Methods::NONE.with(method));
                    continue;
                }
                "pull_request" => {}
                _ => continue,
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
            rules.method_rules += 1;
            rules.listed = rules.listed.and(allowed);
        }
    }
    if pages == 0 {
        return Err("no JSON array");
    }
    Ok(rules)
}

/// The method a `merge_queue` rule merges with: its `parameters.merge_method`.
fn queue_method(
    rule: &serde_json::Map<String, Value>,
) -> std::result::Result<MergeMethod, &'static str> {
    let method = rule
        .get("parameters")
        .and_then(Value::as_object)
        .ok_or("a merge_queue rule has no parameters object")?
        .get("merge_method")
        .and_then(Value::as_str)
        .ok_or("a merge_queue rule has no string merge_method")?;
    match method {
        "MERGE" => Ok(MergeMethod::Merge),
        "SQUASH" => Ok(MergeMethod::Squash),
        "REBASE" => Ok(MergeMethod::Rebase),
        _ => Err("a merge_queue rule's merge_method is not MERGE, SQUASH or REBASE"),
    }
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
        // Rule types that say nothing about the merge method.
        let other = r#"{"type":"deletion"},{"type":"required_signatures"},{"type":"required_status_checks","parameters":{}}"#;
        let linear = r#"{"type":"required_linear_history","ruleset_id":7}"#;
        let plain = r#"{"type":"pull_request","parameters":{"required_approving_review_count":1}}"#;
        let null = r#"{"type":"pull_request","parameters":{"allowed_merge_methods":null}}"#;
        let squash_rebase = methods(false, true, true);
        // (body, pull_request rules listing methods, their intersection,
        // linear history, what the rules allow)
        for (body, method_rules, listed, linear_history, allowed) in [
            ("[]".to_string(), 0, Methods::ALL, false, Methods::ALL),
            ("[]\n[]".to_string(), 0, Methods::ALL, false, Methods::ALL),
            (
                format!("[{other},{plain},{null}]"),
                0,
                Methods::ALL,
                false,
                Methods::ALL,
            ),
            (
                format!(r#"[{}]"#, rule(r#"["merge"]"#)),
                1,
                methods(true, false, false),
                false,
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
                false,
                methods(false, false, true),
            ),
            (
                format!("[{}]", rule("[]")),
                1,
                Methods::NONE,
                false,
                Methods::NONE,
            ),
            (
                format!(r#"[{}]"#, rule(r#"["merge","fast-forward","MERGE"]"#)),
                1,
                methods(true, false, false),
                false,
                methods(true, false, false),
            ),
            // A linear-history rule forbids merge, alone or with others, on
            // any page.
            (format!("[{linear}]"), 0, Methods::ALL, true, squash_rebase),
            (
                format!("[{other},{plain}]\n[{linear}]"),
                0,
                Methods::ALL,
                true,
                squash_rebase,
            ),
            (
                format!(r#"[{linear},{}]"#, rule(r#"["merge","rebase"]"#)),
                1,
                methods(true, false, true),
                true,
                methods(false, false, true),
            ),
            // A pull_request rule that lists only merge, with linear history:
            // nothing is allowed.
            (
                format!(r#"[{}][{linear}]"#, rule(r#"["merge"]"#)),
                1,
                methods(true, false, false),
                true,
                Methods::NONE,
            ),
        ] {
            let got = parse_branch_rules(body.as_bytes()).unwrap();
            assert_eq!(
                got,
                BranchRules {
                    method_rules,
                    listed,
                    linear_history,
                    ..BranchRules::UNLIMITED
                },
                "{body}"
            );
            assert_eq!(got.allowed(), allowed, "{body}");
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
            r#"[{"type":"required_linear_history"},{"type":null}]"#,
            // A merge_queue rule whose method cannot be read is not a rule
            // that leaves the method alone.
            r#"[{"type":"merge_queue"}]"#,
            r#"[{"type":"merge_queue","parameters":[]}]"#,
            r#"[{"type":"merge_queue","parameters":{}}]"#,
            r#"[{"type":"merge_queue","parameters":{"merge_method":null}}]"#,
            r#"[{"type":"merge_queue","parameters":{"merge_method":["SQUASH"]}}]"#,
            r#"[{"type":"merge_queue","parameters":{"merge_method":"squash"}}]"#,
            r#"[{"type":"merge_queue","parameters":{"merge_method":" SQUASH"}}]"#,
            r#"[{"type":"merge_queue","parameters":{"merge_method":"FAST_FORWARD"}}]"#,
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
    fn a_merge_queue_rule_allows_its_own_method_only() {
        let queue = |m: &str| {
            format!(
                r#"{{"type":"merge_queue","ruleset_id":9,"parameters":{{"check_response_timeout_minutes":60,"grouping_strategy":"ALLGREEN","max_entries_to_build":5,"max_entries_to_merge":5,"merge_method":"{m}","min_entries_to_merge":1,"min_entries_to_merge_wait_minutes":5}}}}"#
            )
        };
        let rule = |m: &str| {
            format!(r#"{{"type":"pull_request","parameters":{{"allowed_merge_methods":{m}}}}}"#)
        };
        let linear = r#"{"type":"required_linear_history"}"#;
        // (body, pull_request rules listing methods, their intersection,
        // merge_queue rules, their intersection, linear history, what the
        // rules allow)
        for (body, method_rules, listed, queue_rules, queued, linear_history, allowed) in [
            (
                format!("[{}]", queue("MERGE")),
                0,
                Methods::ALL,
                1,
                methods(true, false, false),
                false,
                methods(true, false, false),
            ),
            (
                format!("[{}]", queue("SQUASH")),
                0,
                Methods::ALL,
                1,
                methods(false, true, false),
                false,
                methods(false, true, false),
            ),
            (
                format!("[{}]", queue("REBASE")),
                0,
                Methods::ALL,
                1,
                methods(false, false, true),
                false,
                methods(false, false, true),
            ),
            // On the second page, beside a pull_request rule that lists the
            // queue's method among others: only the queue's method is left.
            (
                format!("[{}]\n[{}]", rule(r#"["merge","squash"]"#), queue("SQUASH")),
                1,
                methods(true, true, false),
                1,
                methods(false, true, false),
                false,
                methods(false, true, false),
            ),
            // A pull_request rule that does not list the queue's method:
            // nothing is allowed.
            (
                format!("[{},{}]", rule(r#"["rebase"]"#), queue("SQUASH")),
                1,
                methods(false, false, true),
                1,
                methods(false, true, false),
                false,
                Methods::NONE,
            ),
            // A MERGE queue on a linear-history branch: nothing is allowed.
            (
                format!("[{linear},{}]", queue("MERGE")),
                0,
                Methods::ALL,
                1,
                methods(true, false, false),
                true,
                Methods::NONE,
            ),
            (
                format!("[{linear},{}]", queue("REBASE")),
                0,
                Methods::ALL,
                1,
                methods(false, false, true),
                true,
                methods(false, false, true),
            ),
            // Two queues on the branch, from two rulesets, that disagree.
            (
                format!("[{},{}]", queue("SQUASH"), queue("MERGE")),
                0,
                Methods::ALL,
                2,
                Methods::NONE,
                false,
                Methods::NONE,
            ),
            (
                format!("[{}][{}]", queue("REBASE"), queue("REBASE")),
                0,
                Methods::ALL,
                2,
                methods(false, false, true),
                false,
                methods(false, false, true),
            ),
        ] {
            let got = parse_branch_rules(body.as_bytes()).unwrap();
            assert_eq!(
                got,
                BranchRules {
                    method_rules,
                    listed,
                    queue_rules,
                    queued,
                    linear_history,
                },
                "{body}"
            );
            assert_eq!(got.allowed(), allowed, "{body}");
        }
    }

    #[test]
    fn the_rules_name_every_limit_they_apply() {
        let linear = BranchRules {
            linear_history: true,
            ..BranchRules::UNLIMITED
        };
        let listed = BranchRules {
            method_rules: 2,
            listed: methods(true, false, true),
            ..BranchRules::UNLIMITED
        };
        let both = BranchRules {
            linear_history: true,
            ..listed
        };
        let queued = BranchRules {
            queue_rules: 1,
            queued: methods(false, true, false),
            ..BranchRules::UNLIMITED
        };
        let all = BranchRules {
            queue_rules: 1,
            queued: methods(false, false, true),
            ..both
        };
        for (rules, describe, limits) in [
            (
                BranchRules::UNLIMITED,
                "no ruleset on branch main limits the merge method",
                "",
            ),
            (
                linear,
                "a required_linear_history rule on branch main forbids merge",
                "a required_linear_history rule forbids merge",
            ),
            (
                listed,
                "2 ruleset pull_request rule(s) on branch main allow merge, rebase",
                "pull_request allowed_merge_methods allow merge, rebase",
            ),
            (
                both,
                "2 ruleset pull_request rule(s) on branch main allow merge, rebase; \
                 a required_linear_history rule on branch main forbids merge",
                "pull_request allowed_merge_methods allow merge, rebase; \
                 a required_linear_history rule forbids merge",
            ),
            (
                queued,
                "1 ruleset merge_queue rule(s) on branch main allow squash",
                "merge_queue merge_method allows squash",
            ),
            (
                all,
                "2 ruleset pull_request rule(s) on branch main allow merge, rebase; \
                 1 ruleset merge_queue rule(s) on branch main allow rebase; \
                 a required_linear_history rule on branch main forbids merge",
                "pull_request allowed_merge_methods allow merge, rebase; \
                 merge_queue merge_method allows rebase; \
                 a required_linear_history rule forbids merge",
            ),
        ] {
            assert_eq!(rules.describe("main"), describe);
            assert_eq!(rules.limits(), limits);
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
