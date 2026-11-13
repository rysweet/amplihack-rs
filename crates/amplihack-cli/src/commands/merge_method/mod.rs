//! Fail-closed merge-method policy for an authorized auto-drive merge (#1888).
//!
//! The caller is `autodrive-finalize`'s `finalize-observed-action` step,
//! through the launch-bundle helper `autodrive_merge_method.sh`. It is the
//! only place the merge method is decided.
//!
//! The method comes from the first source present, in this order:
//!
//! 1. `explicit`: the recipe's `merge_method` context. The repository file is
//!    then not read, as `AUTODRIVE_QA_COMMAND` stops `.amplihack/qa.toml`
//!    from being read.
//! 2. `repository`: `.amplihack/auto-drive.toml` committed at the base branch
//!    tip the accepted readiness record binds. The merge method is the policy
//!    of the branch that accepts the history, so it is read from that branch
//!    as it stands at merge time, not from the commit the pull request
//!    branched from. A pull request whose changes (merge base to head) touch
//!    the file is refused. The working tree is never read.
//! 3. `default`: the first of squash, merge and rebase that is allowed.
//!
//! A method is allowed when the repository settings (`allow_*`) allow it AND
//! every active ruleset `pull_request` rule on the base branch that lists
//! `allowed_merge_methods` lists it AND, for `merge`, no active ruleset
//! `required_linear_history` rule is on the base branch (GitHub then accepts
//! only squash or rebase). A method that is not allowed is never used, and no
//! source falls back to another. The repository whose settings and rules are
//! read is the one the pull request URL names, as the accepted readiness
//! record binds it.
//! Contract: docs/reference/auto-drive-to-merge.md, "Choosing the merge method".
//!
//! `repo_file` reads the repository file; `github` reads the pull request
//! URL, the repository settings and the branch rulesets.
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::Path;

mod github;
mod repo_file;

pub(crate) use github::PolicyRefusal;
use github::{BranchRules, branch_rules, is_branch, parse_pr_url, repository_settings};
use repo_file::{CONFIG_PATH, repository_method};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}
impl MergeMethod {
    /// The default order, and the order methods are listed in.
    const ORDER: [Self; 3] = [Self::Squash, Self::Merge, Self::Rebase];
    fn parse(value: &str) -> Option<Self> {
        match value {
            "merge" => Some(Self::Merge),
            "squash" => Some(Self::Squash),
            "rebase" => Some(Self::Rebase),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SelectionSource {
    Explicit,
    Repository,
    Default,
}

/// A set of merge methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Methods {
    merge: bool,
    squash: bool,
    rebase: bool,
}
impl Methods {
    const ALL: Self = Self {
        merge: true,
        squash: true,
        rebase: true,
    };
    const NONE: Self = Self {
        merge: false,
        squash: false,
        rebase: false,
    };
    fn allows(self, method: MergeMethod) -> bool {
        match method {
            MergeMethod::Merge => self.merge,
            MergeMethod::Squash => self.squash,
            MergeMethod::Rebase => self.rebase,
        }
    }
    fn with(mut self, method: MergeMethod) -> Self {
        self.set(method, true);
        self
    }
    fn without(mut self, method: MergeMethod) -> Self {
        self.set(method, false);
        self
    }
    fn set(&mut self, method: MergeMethod, value: bool) {
        match method {
            MergeMethod::Merge => self.merge = value,
            MergeMethod::Squash => self.squash = value,
            MergeMethod::Rebase => self.rebase = value,
        }
    }
    fn and(self, other: Self) -> Self {
        Self {
            merge: self.merge && other.merge,
            squash: self.squash && other.squash,
            rebase: self.rebase && other.rebase,
        }
    }
    /// `squash, merge` or `none`, in the default order.
    fn names(self) -> String {
        let names: Vec<&str> = MergeMethod::ORDER
            .into_iter()
            .filter(|m| self.allows(*m))
            .map(MergeMethod::name)
            .collect();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }
}

/// Everything that decides whether a method is allowed.
struct Allowed<'a> {
    settings: Methods,
    rules: BranchRules,
    base_ref: &'a str,
}

/// What the selector reports. `config_commit` is the base branch tip the
/// repository file was read at, or `None` when an explicit method meant it
/// was not read.
#[derive(Debug, Serialize)]
pub struct MergeSelection {
    method: MergeMethod,
    source: SelectionSource,
    repository: String,
    config_commit: Option<String>,
    rationale: String,
}

/// What the finalisation step passes in.
pub struct SelectionRequest<'a> {
    /// The pull request's `url`, as `gh pr view` reported it.
    pub pr_url: &'a str,
    /// The checkout whose object store holds `base` and `head`.
    pub repo_root: &'a Path,
    /// The pull request's base branch name (`baseRefName`); its rulesets are
    /// read whatever the source.
    pub base_ref: &'a str,
    /// The base branch tip the accepted readiness record binds.
    pub base: &'a str,
    /// The pull request head (`headRefOid`).
    pub head: &'a str,
    /// The recipe's `merge_method` context; empty when not given.
    pub explicit: &'a str,
}

/// Select an allowed merge method, or say exactly why none can be chosen.
pub fn select(request: &SelectionRequest<'_>) -> Result<MergeSelection> {
    let repository = parse_pr_url(request.pr_url)?;
    anyhow::ensure!(
        is_branch(request.base_ref),
        "the base branch name is empty or not a valid branch name, so the rulesets that limit its merge methods cannot be read"
    );
    let explicit = if request.explicit.is_empty() {
        None
    } else {
        Some(
            MergeMethod::parse(request.explicit)
                .context("merge_method context must be exactly merge, squash or rebase")?,
        )
    };
    let (configured, config_commit) = match explicit {
        Some(_) => (None, None),
        None => (
            repository_method(request.repo_root, request.base, request.head)?,
            Some(request.base.to_string()),
        ),
    };
    let allowed = Allowed {
        settings: repository_settings(&repository)?,
        rules: branch_rules(&repository, request.base_ref)?,
        base_ref: request.base_ref,
    };
    let (method, source, reason) = choose(explicit, configured, &allowed)?;
    let file = match (&config_commit, configured) {
        (Some(base), Some(_)) => format!("{CONFIG_PATH} read at base tip {base}"),
        (Some(base), None) => format!("no merge_method in {CONFIG_PATH} at base tip {base}"),
        (None, _) => format!("{CONFIG_PATH} not read"),
    };
    let rules = allowed.rules.describe(request.base_ref);
    Ok(MergeSelection {
        method,
        source,
        repository: format!("{}/{}", repository.owner, repository.name),
        config_commit,
        rationale: format!(
            "Selected {} from {reason}; {file}; repository settings allow it; {rules}",
            method.name()
        ),
    })
}

fn choose(
    explicit: Option<MergeMethod>,
    configured: Option<MergeMethod>,
    allowed: &Allowed<'_>,
) -> Result<(MergeMethod, SelectionSource, &'static str)> {
    let by_rules = allowed.rules.allowed();
    let both = allowed.settings.and(by_rules);
    let (method, source, reason) = if let Some(method) = explicit {
        (
            method,
            SelectionSource::Explicit,
            "the explicit merge_method context",
        )
    } else if let Some(method) = configured {
        (
            method,
            SelectionSource::Repository,
            "the repository's merge_method",
        )
    } else {
        let Some(method) = MergeMethod::ORDER.into_iter().find(|m| both.allows(*m)) else {
            if allowed.settings == Methods::NONE {
                bail!("the repository allows no merge method");
            }
            bail!(
                "the repository settings allow {} and the rulesets on branch {} allow {}: no merge method is allowed by both ({})",
                allowed.settings.names(),
                allowed.base_ref,
                by_rules.names(),
                allowed.rules.limits()
            );
        };
        (
            method,
            SelectionSource::Default,
            "the first allowed method in squash, merge, rebase order",
        )
    };
    anyhow::ensure!(
        allowed.settings.allows(method),
        "selected merge method {} is disallowed by repository permissions; no other method is tried",
        method.name()
    );
    anyhow::ensure!(
        by_rules.allows(method),
        "selected merge method {} is disallowed by the rulesets on branch {} ({}); no other method is tried",
        method.name(),
        allowed.base_ref,
        allowed.rules.limits()
    );
    Ok((method, source, reason))
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
    fn unrestricted(settings: Methods) -> Allowed<'static> {
        Allowed {
            settings,
            rules: BranchRules::UNLIMITED,
            base_ref: "main",
        }
    }

    #[test]
    fn defaults_are_deterministic_for_all_permission_combinations() {
        for mask in 0..8 {
            let a = methods(mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
            let result = choose(None, None, &unrestricted(a));
            if mask == 0 {
                assert!(result.is_err());
                continue;
            }
            let (method, source, _) = result.unwrap();
            assert_eq!(source, SelectionSource::Default);
            let want = if mask & 2 != 0 {
                MergeMethod::Squash
            } else if mask & 1 != 0 {
                MergeMethod::Merge
            } else {
                MergeMethod::Rebase
            };
            assert_eq!(method, want);
        }
    }

    #[test]
    fn the_default_is_the_first_method_settings_and_rules_both_allow() {
        // Every settings mask against every rules mask, with and without a
        // linear-history rule: the default is the first of squash, merge,
        // rebase that the settings allow, the pull_request rules list and,
        // for merge, linear history does not forbid; or an error.
        for linear_history in [false, true] {
            for s in 0..8 {
                for r in 0..8 {
                    let settings = methods(s & 1 != 0, s & 2 != 0, s & 4 != 0);
                    let listed = methods(r & 1 != 0, r & 2 != 0, r & 4 != 0);
                    let allowed = Allowed {
                        settings,
                        rules: BranchRules {
                            method_rules: 1,
                            listed,
                            linear_history,
                        },
                        base_ref: "main",
                    };
                    let want = MergeMethod::ORDER.into_iter().find(|m| {
                        settings.allows(*m)
                            && listed.allows(*m)
                            && !(linear_history && *m == MergeMethod::Merge)
                    });
                    let case = format!("settings {s} rules {r} linear {linear_history}");
                    match (choose(None, None, &allowed), want) {
                        (Ok((method, SelectionSource::Default, _)), Some(want)) => {
                            assert_eq!(method, want, "{case}")
                        }
                        (Err(e), None) => {
                            let message = format!("{e:#}");
                            assert!(
                                message.contains("allows no merge method")
                                    || message.contains("no merge method is allowed by both"),
                                "{case}: {message}"
                            );
                            assert_eq!(
                                linear_history && settings != Methods::NONE,
                                message.contains("a required_linear_history rule forbids merge"),
                                "{case}: {message}"
                            );
                        }
                        (other, want) => panic!("{case}: {other:?} want {want:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn linear_history_forbids_merge_from_every_source() {
        let linear = |settings| Allowed {
            settings,
            rules: BranchRules {
                linear_history: true,
                ..BranchRules::UNLIMITED
            },
            base_ref: "main",
        };
        // Settings allow merge and rebase, not squash: the default skips
        // merge, which GitHub would refuse, and takes rebase.
        let (method, source, _) = choose(None, None, &linear(methods(true, false, true))).unwrap();
        assert_eq!(
            (method, source),
            (MergeMethod::Rebase, SelectionSource::Default)
        );
        // An explicit or repository merge is refused and names the rule.
        for (explicit, configured) in [
            (Some(MergeMethod::Merge), None),
            (None, Some(MergeMethod::Merge)),
        ] {
            let message = format!(
                "{:#}",
                choose(explicit, configured, &linear(Methods::ALL)).unwrap_err()
            );
            assert_eq!(
                message,
                "selected merge method merge is disallowed by the rulesets on branch main \
                 (a required_linear_history rule forbids merge); no other method is tried"
            );
        }
        // Squash and rebase stay allowed.
        for method in [MergeMethod::Squash, MergeMethod::Rebase] {
            let (got, _, _) = choose(Some(method), None, &linear(Methods::ALL)).unwrap();
            assert_eq!(got, method);
        }
        // Settings that allow only merge leave nothing.
        let message = format!(
            "{:#}",
            choose(None, None, &linear(methods(true, false, false))).unwrap_err()
        );
        assert_eq!(
            message,
            "the repository settings allow merge and the rulesets on branch main allow squash, \
             rebase: no merge method is allowed by both (a required_linear_history rule forbids merge)"
        );
    }

    #[test]
    fn explicit_wins_over_repository_preference() {
        let (method, source, _) = choose(
            Some(MergeMethod::Merge),
            Some(MergeMethod::Rebase),
            &unrestricted(methods(true, false, false)),
        )
        .unwrap();
        assert_eq!(method, MergeMethod::Merge);
        assert_eq!(source, SelectionSource::Explicit);
    }

    #[test]
    fn disallowed_preferences_never_fall_back() {
        for explicit in [false, true] {
            let result = choose(
                explicit.then_some(MergeMethod::Merge),
                Some(MergeMethod::Merge),
                &unrestricted(methods(false, true, true)),
            );
            let message = format!("{:#}", result.unwrap_err());
            assert!(message.contains("merge is disallowed"), "{message}");
        }
        // A ruleset that does not list the method refuses it too, even when
        // the settings allow it and another method would do.
        let ruled = Allowed {
            settings: Methods::ALL,
            rules: BranchRules {
                method_rules: 1,
                listed: methods(true, false, false),
                linear_history: false,
            },
            base_ref: "main",
        };
        for (explicit, configured) in [
            (Some(MergeMethod::Squash), None),
            (None, Some(MergeMethod::Squash)),
        ] {
            let message = format!("{:#}", choose(explicit, configured, &ruled).unwrap_err());
            assert!(
                message.contains("squash is disallowed by the rulesets on branch main"),
                "{message}"
            );
        }
    }
}
