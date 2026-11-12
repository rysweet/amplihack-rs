//! Fail-closed merge-method policy for the trusted auto-drive merge gate (#1888).
//!
//! The method comes from the first source present, in this order:
//!
//! 1. `explicit`: the recipe's `merge_method` context. The repository file is
//!    then not read, as `AUTODRIVE_QA_COMMAND` stops `.amplihack/qa.toml`
//!    from being read.
//! 2. `repository`: `.amplihack/auto-drive.toml` committed at the merge base
//!    of the pull request head and its fetched base branch. A pull request
//!    that changes the file is refused. The working tree is never read.
//! 3. `default`: the first of squash, merge and rebase the repository allows.
//!
//! A method the repository does not allow is never used, and no source falls
//! back to another. The repository whose permissions are read is the one the
//! pull request URL names: the gate already read that URL from `gh pr view`.
//! Contract: docs/reference/auto-drive-to-merge.md, "Choosing the merge method".
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{path::Path, process::Command};

/// The repository file, relative to the repository root.
pub const CONFIG_PATH: &str = ".amplihack/auto-drive.toml";
const CONFIG_DIR: &str = ".amplihack";
/// Larger files are refused unread, as `.amplihack/qa.toml` is.
const CONFIG_MAX_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}
impl MergeMethod {
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

#[derive(Debug, serde::Deserialize)]
pub struct AllowedMergeMethods {
    allow_merge_commit: bool,
    allow_squash_merge: bool,
    allow_rebase_merge: bool,
}
impl AllowedMergeMethods {
    fn allows(&self, method: MergeMethod) -> bool {
        match method {
            MergeMethod::Merge => self.allow_merge_commit,
            MergeMethod::Squash => self.allow_squash_merge,
            MergeMethod::Rebase => self.allow_rebase_merge,
        }
    }
}

/// What the gate records. `config_commit` is the merge base the repository
/// file was read at, or `None` when an explicit method meant it was not read.
#[derive(Debug, Serialize)]
pub struct MergeSelection {
    method: MergeMethod,
    source: SelectionSource,
    repository: String,
    config_commit: Option<String>,
    rationale: String,
}

#[derive(Debug, thiserror::Error)]
#[error("gh api refused repository merge permissions (terminal policy exit 79)")]
pub(super) struct PolicyRefusal;

/// The pull request's repository, from its URL `https://HOST/OWNER/REPO/pull/N`.
#[derive(Debug, PartialEq, Eq)]
struct Repository {
    host: String,
    owner: String,
    name: String,
}

fn parse_pr_url(url: &str) -> Result<Repository> {
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

fn is_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// git in `root` with no inherited repository selection (`amplihack_git`,
/// #1278), replace refs, grafts or prompts: the environment
/// `autodrive_trust_git` gives the gate's own reads.
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = amplihack_git::command_in(root)
        .env_remove("GIT_REPLACE_REF_BASE")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .context("cannot execute git")?;
    anyhow::ensure!(output.status.success(), "git {} failed", args[0]);
    Ok(output.stdout)
}

/// `<mode> <type> <oid> <size>` of `path` in `tree`, or `None` when absent.
fn tree_entry(
    root: &Path,
    tree: &str,
    path: &str,
) -> Result<Option<(String, String, String, String)>> {
    let out = git(
        root,
        &[
            "ls-tree",
            "-z",
            "--long",
            "--full-tree",
            "--end-of-options",
            tree,
            "--",
            path,
        ],
    )?;
    let mut entries = out.split(|b| *b == 0).filter(|e| !e.is_empty());
    let Some(entry) = entries.next() else {
        return Ok(None);
    };
    anyhow::ensure!(
        entries.next().is_none(),
        "git ls-tree listed more than {path}"
    );
    let entry = std::str::from_utf8(entry).context("git ls-tree output is not UTF-8")?;
    let (meta, listed) = entry
        .split_once('\t')
        .context("git ls-tree output has no path")?;
    anyhow::ensure!(listed == path, "git ls-tree listed a different path");
    let mut fields = meta.split_whitespace();
    let mut next = || fields.next().map(str::to_string).unwrap_or_default();
    Ok(Some((next(), next(), next(), next())))
}

/// The configured method, read from the merge base of `base` and `head`.
/// Returns the merge base it read at; `None` for the method when the file is
/// not present there. Refuses a pull request that changes the file.
fn repository_method(root: &Path, base: &str, head: &str) -> Result<(Option<MergeMethod>, String)> {
    anyhow::ensure!(
        is_sha(base),
        "the base branch was not fetched, so the merge base that {CONFIG_PATH} is read at is unknown"
    );
    anyhow::ensure!(
        is_sha(head),
        "the pull request head is not a full commit SHA"
    );
    let commit = |sha: &str| -> Result<()> {
        let out = git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{sha}^{{commit}}"),
            ],
        )
        .with_context(|| format!("commit {sha} is not in the local clone"))?;
        anyhow::ensure!(
            String::from_utf8_lossy(&out).trim() == sha,
            "commit {sha} is not in the local clone"
        );
        Ok(())
    };
    commit(base)?;
    commit(head)?;
    let mb = git(root, &["merge-base", "--end-of-options", base, head])
        .context("the pull request head and its base have no merge base")?;
    let mb = String::from_utf8(mb).context("git merge-base output is not UTF-8")?;
    let mb = mb.trim().to_string();
    anyhow::ensure!(is_sha(&mb), "git merge-base gave no commit");

    let changed = git(
        root,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "-z",
            "--end-of-options",
            &mb,
            head,
        ],
    )
    .context("the paths the pull request changes cannot be read")?;
    if changed
        .split(|b| *b == 0)
        .any(|p| p == CONFIG_DIR.as_bytes() || p == CONFIG_PATH.as_bytes())
    {
        bail!(
            "{CONFIG_PATH}: changed-in-pr: the pull request changes {CONFIG_PATH}, and a pull request never chooses its own merge method; merge the change with an explicit merge_method"
        );
    }

    let invalid = |problem: &str| anyhow::anyhow!("{CONFIG_PATH} at merge base {mb}: {problem}");
    match tree_entry(root, &mb, CONFIG_DIR)? {
        None => return Ok((None, mb)),
        Some((mode, kind, _, _)) => {
            if mode == "120000" {
                return Err(invalid("symlink"));
            }
            if mode != "040000" || kind != "tree" {
                return Err(invalid("not-regular"));
            }
        }
    }
    let (oid, size) = match tree_entry(root, &mb, CONFIG_PATH)? {
        None => return Ok((None, mb)),
        Some((mode, _, _, _)) if mode == "120000" => return Err(invalid("symlink")),
        Some((mode, kind, oid, size))
            if (mode == "100644" || mode == "100755") && kind == "blob" =>
        {
            (oid, size)
        }
        Some(_) => return Err(invalid("not-regular")),
    };
    let size: usize = size.parse().map_err(|_| invalid("not-regular"))?;
    if size > CONFIG_MAX_BYTES {
        return Err(invalid("too-large"));
    }
    let body = git(root, &["cat-file", "blob", &oid])?;
    if body.len() != size {
        return Err(invalid("not-regular"));
    }
    let method = parse_config(&body).map_err(invalid)?;
    Ok((method, mb))
}

/// The `.amplihack/qa.toml` grammar: blank lines, `#` comments and
/// `key = "value"` lines of printable ASCII. Only `merge_method` is a key.
/// Errors are fixed tokens, so no file content reaches the evidence.
fn parse_config(body: &[u8]) -> std::result::Result<Option<MergeMethod>, &'static str> {
    if !body
        .iter()
        .all(|b| matches!(b, b'\t' | b'\n' | b' '..=b'~'))
    {
        return Err("syntax");
    }
    let text = std::str::from_utf8(body).map_err(|_| "syntax")?;
    let mut method = None;
    for line in text.lines() {
        let line = line.trim_start_matches([' ', '\t']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key_len = line
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            .count();
        let (key, rest) = line.split_at(key_len);
        let rest = rest.trim_start_matches([' ', '\t']);
        let rest = rest.strip_prefix('=').ok_or("syntax")?;
        let rest = rest.trim_start_matches([' ', '\t']);
        let rest = rest.strip_prefix('"').ok_or("syntax")?;
        let (value, tail) = rest.split_once('"').ok_or("syntax")?;
        if key.is_empty() || value.contains('\\') || !tail.trim_matches([' ', '\t']).is_empty() {
            return Err("syntax");
        }
        if key != "merge_method" {
            return Err("unknown-key");
        }
        if method.is_some() {
            return Err("duplicate-key");
        }
        method = Some(MergeMethod::parse(value).ok_or("invalid-merge-method")?);
    }
    Ok(method)
}

fn repository_permissions(repository: &Repository) -> Result<AllowedMergeMethods> {
    let output = Command::new("gh")
        .args([
            "api",
            "--hostname",
            &repository.host,
            &format!("repos/{}/{}", repository.owner, repository.name),
        ])
        .output()
        .context("cannot execute gh api for merge permissions")?;
    if output.status.code() == Some(79) {
        return Err(PolicyRefusal.into());
    }
    anyhow::ensure!(
        output.status.success(),
        "gh api could not read repository merge permissions (status {})",
        output.status
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("invalid repository merge permissions JSON")?;
    anyhow::ensure!(
        response.is_object(),
        "repository merge permissions must be an object"
    );
    serde_json::from_value(response)
        .context("invalid repository merge permissions: all three allow_* fields must be booleans")
}

/// What the gate passes in.
pub struct SelectionRequest<'a> {
    /// The pull request's `url`, as `gh pr view` reported it.
    pub pr_url: &'a str,
    /// The checkout whose object store holds `base` and `head`.
    pub repo_root: &'a Path,
    /// The base branch tip, fetched by the gate (`autodrive_base_sha`).
    pub base: &'a str,
    /// The pull request head (`headRefOid`).
    pub head: &'a str,
    /// The recipe's `merge_method` context; empty when not given.
    pub explicit: &'a str,
}

/// Select an allowed merge method, or say exactly why none can be chosen.
pub fn select(request: &SelectionRequest<'_>) -> Result<MergeSelection> {
    let repository = parse_pr_url(request.pr_url)?;
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
        None => {
            let (method, mb) = repository_method(request.repo_root, request.base, request.head)?;
            (method, Some(mb))
        }
    };
    let allowed = repository_permissions(&repository)?;
    let (method, source, reason) = choose(explicit, configured, &allowed)?;
    let reason = match (&config_commit, configured) {
        (Some(mb), Some(_)) => format!("{reason}; {CONFIG_PATH} read at merge base {mb}"),
        (Some(mb), None) => {
            format!("{reason}; no merge_method in {CONFIG_PATH} at merge base {mb}")
        }
        (None, _) => format!("{reason}; {CONFIG_PATH} not read"),
    };
    Ok(MergeSelection {
        method,
        source,
        repository: format!("{}/{}", repository.owner, repository.name),
        config_commit,
        rationale: format!(
            "Selected {} from {reason}; repository permissions allow it",
            method.name()
        ),
    })
}

fn choose(
    explicit: Option<MergeMethod>,
    configured: Option<MergeMethod>,
    allowed: &AllowedMergeMethods,
) -> Result<(MergeMethod, SelectionSource, &'static str)> {
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
        let method = [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase]
            .into_iter()
            .find(|m| allowed.allows(*m))
            .context("the repository allows no merge method")?;
        (
            method,
            SelectionSource::Default,
            "the first allowed method in squash, merge, rebase order",
        )
    };
    anyhow::ensure!(
        allowed.allows(method),
        "selected merge method {} is disallowed by repository permissions; no other method is tried",
        method.name()
    );
    Ok((method, source, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(merge: bool, squash: bool, rebase: bool) -> AllowedMergeMethods {
        AllowedMergeMethods {
            allow_merge_commit: merge,
            allow_squash_merge: squash,
            allow_rebase_merge: rebase,
        }
    }

    #[test]
    fn defaults_are_deterministic_for_all_permission_combinations() {
        for mask in 0..8 {
            let a = allowed(mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
            let result = choose(None, None, &a);
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
    fn explicit_wins_over_repository_preference() {
        let (method, source, _) = choose(
            Some(MergeMethod::Merge),
            Some(MergeMethod::Rebase),
            &allowed(true, false, false),
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
                &allowed(false, true, true),
            );
            let message = format!("{:#}", result.unwrap_err());
            assert!(message.contains("merge is disallowed"), "{message}");
        }
    }

    #[test]
    fn config_grammar_matches_qa_toml() {
        for (text, want) in [
            ("", Ok(None)),
            ("# only a comment\n\n", Ok(None)),
            ("merge_method = \"merge\"\n", Ok(Some(MergeMethod::Merge))),
            (
                "  merge_method\t=\t\"rebase\"  \n",
                Ok(Some(MergeMethod::Rebase)),
            ),
            ("merge_method=\"squash\"", Ok(Some(MergeMethod::Squash))),
            ("merge_method = \"MERGE\"\n", Err("invalid-merge-method")),
            ("merge_method = \"\"\n", Err("invalid-merge-method")),
            ("merge_method = merge\n", Err("syntax")),
            ("merge_method: merge\n", Err("syntax")),
            ("merge_method = \"merge\" # trailing\n", Err("syntax")),
            ("merge_method = \"me\\rge\"\n", Err("syntax")),
            ("merge_method = \"merge\"\r\n", Err("syntax")),
            ("command = \"cargo test\"\n", Err("unknown-key")),
            (
                "merge_method = \"merge\"\nmerge_method = \"merge\"\n",
                Err("duplicate-key"),
            ),
            ("[section]\n", Err("syntax")),
            ("merge_method = \"m\u{e9}rge\"\n", Err("syntax")),
        ] {
            assert_eq!(parse_config(text.as_bytes()), want, "{text:?}");
        }
    }

    #[test]
    fn pr_url_names_the_repository() {
        assert_eq!(
            parse_pr_url("https://github.com/rysweet/Simard/pull/5038").unwrap(),
            Repository {
                host: "github.com".into(),
                owner: "rysweet".into(),
                name: "Simard".into()
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
    fn shas_are_full_lowercase_hex() {
        assert!(is_sha(&"a".repeat(40)));
        assert!(is_sha(&"0".repeat(64)));
        assert!(!is_sha(&"A".repeat(40)));
        assert!(!is_sha(&"a".repeat(39)));
        assert!(!is_sha("HEAD"));
        assert!(!is_sha(""));
    }
}
