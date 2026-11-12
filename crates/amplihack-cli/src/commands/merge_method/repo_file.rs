//! The repository file `.amplihack/auto-drive.toml` (#1888), read from git
//! objects only: the base branch tip's tree, and the diff from the merge base
//! to the pull request head. Same grammar, trust checks and tokens as the
//! `.amplihack/qa.toml` reader in `autodrive_qa_sources.sh`; the parity test
//! in `tests/issue_1888_merge_method_selection.rs` runs both over one set of
//! fixtures. Contract: docs/reference/auto-drive-to-merge.md, "The
//! repository file `.amplihack/auto-drive.toml`".
use super::MergeMethod;
use anyhow::{Context, Result, bail};
use std::path::Path;

/// The repository file, relative to the repository root.
pub(super) const CONFIG_PATH: &str = ".amplihack/auto-drive.toml";
const CONFIG_DIR: &str = ".amplihack";
/// Larger files are refused unread, as `.amplihack/qa.toml` is.
const CONFIG_MAX_BYTES: usize = 4096;

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

/// The configured method, read at the base branch tip `base`. `None` when
/// the file is not present there.
///
/// The checks run in the order `autodrive_qa_config` runs them for
/// `.amplihack/qa.toml` (`autodrive_qa_sources.sh`), with the same tokens:
/// present (`.amplihack` is anything but a directory, or the file has an
/// entry) or changed, then `paths-empty`, `changed-in-pr`, the two tree
/// entries, the size and the grammar. The changed paths are the diff from the
/// merge base of `base` and `head` to `head`: what the pull request changes.
/// The parity test in `tests/issue_1888_merge_method_selection.rs` runs both
/// readers over one fixture set. Two differences are deliberate and
/// documented: qa.toml is read at the merge base, because it says how to test
/// the pull request's code, and changed paths that cannot be read refuse here
/// even when the file is absent.
pub(super) fn repository_method(
    root: &Path,
    base: &str,
    head: &str,
) -> Result<Option<MergeMethod>> {
    anyhow::ensure!(
        is_sha(base),
        "the base branch was not fetched, so the base branch tip that {CONFIG_PATH} is read at is unknown"
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
    let paths: Vec<&[u8]> = changed
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .collect();
    let touched = paths
        .iter()
        .any(|p| *p == CONFIG_DIR.as_bytes() || *p == CONFIG_PATH.as_bytes());

    let invalid = |problem: &str| anyhow::anyhow!("{CONFIG_PATH} at base tip {base}: {problem}");
    let dir = tree_entry(root, base, CONFIG_DIR)?;
    let file = tree_entry(root, base, CONFIG_PATH)?;
    let present = match &dir {
        None => false,
        Some((mode, _, _, _)) if mode == "040000" => file.is_some(),
        Some(_) => true,
    };
    if !present && !touched {
        return Ok(None);
    }
    // The file is used only when the diff shows the pull request left it
    // alone, and an empty diff shows nothing (qa.toml's `paths-empty`).
    if paths.is_empty() {
        bail!(
            "{CONFIG_PATH}: paths-empty: the pull request changes no file between merge base {mb} and its head, so nothing shows it leaves {CONFIG_PATH} alone; merge it with an explicit merge_method"
        );
    }
    if touched {
        bail!(
            "{CONFIG_PATH}: changed-in-pr: the pull request changes {CONFIG_PATH}, and a pull request never chooses its own merge method; merge the change with an explicit merge_method"
        );
    }
    match &dir {
        Some((mode, _, _, _)) if mode == "120000" => return Err(invalid("symlink")),
        Some((mode, kind, _, _)) if mode == "040000" && kind == "tree" => {}
        _ => return Err(invalid("not-regular")),
    }
    let (oid, size) = match file {
        Some((mode, _, _, _)) if mode == "120000" => return Err(invalid("symlink")),
        Some((mode, kind, oid, size))
            if (mode == "100644" || mode == "100755") && kind == "blob" =>
        {
            (oid, size)
        }
        _ => return Err(invalid("not-regular")),
    };
    let size: usize = size.parse().map_err(|_| invalid("not-regular"))?;
    if size > CONFIG_MAX_BYTES {
        return Err(invalid("too-large"));
    }
    let body = git(root, &["cat-file", "blob", &oid])?;
    if body.len() != size {
        return Err(invalid("not-regular"));
    }
    parse_config(&body).map_err(invalid)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn shas_are_full_lowercase_hex() {
        assert!(is_sha(&"a".repeat(40)));
        assert!(is_sha(&"0".repeat(64)));
        assert!(!is_sha(&"A".repeat(40)));
        assert!(!is_sha(&"a".repeat(39)));
        assert!(!is_sha("HEAD"));
        assert!(!is_sha(""));
    }
}
