//! Authenticated hosting observations and narrow handoff effects. No merge capability.
use super::{
    records::{Binding, PlatformState},
    validation::{MAX_RECORD_BYTES, validate_binding},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

#[derive(Debug, Clone)]
pub enum BaseConfig {
    Missing,
    File { bytes: Vec<u8>, mode: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrObservation {
    pub binding: Binding,
    pub state: PlatformState,
    pub draft: bool,
    pub auto_merge_enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    pub id: u64,
    pub pr_url: String,
    pub author: String,
    pub body: String,
}
#[derive(Debug, Clone)]
pub struct CommentPage {
    pub comments: Vec<Comment>,
    pub next: Option<String>,
}
pub trait PolicyHosting {
    fn observe_pr(&mut self, binding: &Binding) -> Result<PrObservation>;
    fn base_tip(&mut self, binding: &Binding) -> Result<String>;
    fn read_base_config(
        &mut self,
        binding: &Binding,
        commit: &str,
        path: &str,
    ) -> Result<BaseConfig>;
}
pub trait HandoffHosting: PolicyHosting {
    fn mark_ready(&mut self, binding: &Binding) -> Result<()>;
    fn authenticated_login(&mut self) -> Result<String>;
    fn list_comments(&mut self, binding: &Binding, cursor: Option<&str>) -> Result<CommentPage>;
    fn read_comment(&mut self, binding: &Binding, id: u64) -> Result<Comment>;
    fn create_comment(&mut self, binding: &Binding, body: &str) -> Result<Comment>;
}

pub struct Github {
    host: String,
}
impl Github {
    pub fn new(binding: &Binding) -> Result<Self> {
        validate_binding(binding)?;
        ensure!(
            binding.platform == "github"
                && binding
                    .host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                && [&binding.repository, &binding.source_repository]
                    .iter()
                    .all(|s| s.split('/').all(|p| !p.is_empty()
                        && !matches!(p, "." | "..")
                        && p.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))),
            "hosting-identity-invalid: unsupported host or repository"
        );
        Ok(Self {
            host: binding.host.clone(),
        })
    }
    fn bytes(&self, endpoint: &str, body: Option<Value>, raw: bool) -> Result<Vec<u8>> {
        let mut cmd = Command::new("gh");
        cmd.args([
            "api",
            "--hostname",
            &self.host,
            "--method",
            if body.is_some() { "POST" } else { "GET" },
        ]);
        if raw {
            cmd.args(["--header", "Accept: application/vnd.github.raw+json"]);
        }
        if body.is_some() {
            cmd.args(["--input", "-"]);
        }
        cmd.arg(endpoint)
            .stdin(if body.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Finite API observations, with no retries or scheduling. Do not retain credential-bearing stderr.
        let mut child = cmd
            .spawn()
            .context("hosting-unavailable: cannot start authenticated gh")?;
        if let Some(body) = body {
            child
                .stdin
                .take()
                .context("hosting-stdin")?
                .write_all(&serde_json::to_vec(&body)?)?;
        }
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .context("hosting-stdout")?
            .take(MAX_RECORD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_RECORD_BYTES {
            child
                .kill()
                .context("hosting-oversize: stop oversized response")?;
            child.wait()?;
            anyhow::bail!("hosting-oversize: response exceeds size limit");
        }
        let status = child.wait()?;
        ensure!(
            status.success(),
            "hosting-request-failed: authenticated API request exited {status} for {endpoint}"
        );
        Ok(bytes)
    }
    fn api(&self, endpoint: &str, body: Option<Value>) -> Result<Value> {
        serde_json::from_slice(&self.bytes(endpoint, body, false)?)
            .context("hosting-json: invalid API response")
    }
    fn pull(&self, b: &Binding) -> Result<Value> {
        ensure!(b.host == self.host, "hosting-host-mismatch");
        let pr = self.api(
            &format!("repos/{}/pulls/{}", b.repository, b.pr_number),
            None,
        )?;
        ensure!(
            pr["number"].as_u64() == Some(b.pr_number)
                && string(&pr, "html_url")? == b.pr_url
                && string(&pr["base"]["repo"], "full_name")? == b.repository,
            "hosting-pr-identity-mismatch"
        );
        Ok(pr)
    }
    fn tree(&self, repository: &str, sha: &str) -> Result<Vec<Value>> {
        ensure!(valid_sha(sha), "hosting-tree-sha-invalid");
        let tree = self.api(&format!("repos/{repository}/git/trees/{sha}"), None)?;
        ensure!(
            tree["truncated"] == false && string(&tree, "sha")? == sha,
            "hosting-tree-incomplete"
        );
        Ok(tree["tree"]
            .as_array()
            .context("hosting-tree-missing")?
            .clone())
    }
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .with_context(|| format!("hosting-field-missing: {key}"))
}
fn valid_sha(s: &str) -> bool {
    [40, 64].contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn segment(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            result.push(b as char);
        } else {
            result.push_str(&format!("%{b:02X}"));
        }
    }
    result
}
fn comment(v: &Value, b: &Binding) -> Result<Comment> {
    let id = v["id"]
        .as_u64()
        .filter(|id| *id > 0)
        .context("hosting-comment-id")?;
    ensure!(
        string(v, "issue_url")?
            == format!(
                "https://{}/repos/{}/issues/{}",
                if b.host == "github.com" {
                    "api.github.com".to_owned()
                } else {
                    format!("{}/api/v3", b.host)
                },
                b.repository,
                b.pr_number
            ),
        "hosting-comment-pr-mismatch"
    );
    Ok(Comment {
        id,
        pr_url: b.pr_url.clone(),
        author: string(&v["user"], "login")?.into(),
        body: string(v, "body")?.into(),
    })
}
impl PolicyHosting for Github {
    fn observe_pr(&mut self, b: &Binding) -> Result<PrObservation> {
        let pr = self.pull(b)?;
        ensure!(
            pr["number"].as_u64() == Some(b.pr_number) && string(&pr, "html_url")? == b.pr_url,
            "hosting-pr-mismatch"
        );
        let mut binding = b.clone();
        binding.repository = string(&pr["base"]["repo"], "full_name")?.into();
        binding.source_repository = string(&pr["head"]["repo"], "full_name")?.into();
        binding.head_branch = string(&pr["head"], "ref")?.into();
        binding.base_branch = string(&pr["base"], "ref")?.into();
        binding.head_sha = string(&pr["head"], "sha")?.into();
        binding.base_sha = string(&pr["base"], "sha")?.into();
        let state = if pr["merged"] == true {
            PlatformState::Merged
        } else {
            match string(&pr, "state")? {
                "open" => PlatformState::Open,
                "closed" => PlatformState::Closed,
                _ => anyhow::bail!("hosting-pr-state-invalid"),
            }
        };
        let auto_merge = pr
            .get("auto_merge")
            .context("hosting-auto-merge-status-missing")?;
        Ok(PrObservation {
            binding,
            state,
            draft: pr["draft"]
                .as_bool()
                .context("hosting-draft-status-missing")?,
            auto_merge_enabled: !auto_merge.is_null(),
        })
    }
    fn base_tip(&mut self, b: &Binding) -> Result<String> {
        let value = self.api(
            &format!(
                "repos/{}/branches/{}",
                b.repository,
                segment(&b.base_branch)
            ),
            None,
        )?;
        let sha = string(&value["commit"], "sha")?;
        ensure!(
            string(&value, "name")? == b.base_branch,
            "hosting-base-branch-mismatch"
        );
        ensure!(valid_sha(sha), "hosting-base-sha-invalid");
        Ok(sha.into())
    }
    fn read_base_config(&mut self, b: &Binding, commit: &str, path: &str) -> Result<BaseConfig> {
        ensure!(
            path == ".amplihack/auto-drive.toml" && valid_sha(commit),
            "hosting-config-path-invalid"
        );
        let commit_data = self.api(
            &format!("repos/{}/git/commits/{commit}", b.repository),
            None,
        )?;
        ensure!(
            string(&commit_data, "sha")? == commit,
            "hosting-commit-mismatch"
        );
        let root = self.tree(&b.repository, string(&commit_data["tree"], "sha")?)?;
        let Some(directory) = root.iter().find(|e| e["path"] == ".amplihack") else {
            return Ok(BaseConfig::Missing);
        };
        ensure!(
            directory["mode"] == "040000" && directory["type"] == "tree",
            "hosting-config-directory-unsuitable"
        );
        let entries = self.tree(&b.repository, string(directory, "sha")?)?;
        let Some(file) = entries.iter().find(|e| e["path"] == "auto-drive.toml") else {
            return Ok(BaseConfig::Missing);
        };
        let mode = string(file, "mode")?;
        ensure!(
            file["type"] == "blob"
                && matches!(mode, "100644" | "100755")
                && file["size"]
                    .as_u64()
                    .is_some_and(|n| n <= MAX_RECORD_BYTES as u64),
            "hosting-config-entry-unsuitable"
        );
        let sha = string(file, "sha")?;
        ensure!(valid_sha(sha), "hosting-blob-sha-invalid");
        let bytes = self.bytes(
            &format!("repos/{}/git/blobs/{sha}", b.repository),
            None,
            true,
        )?;
        Ok(BaseConfig::File {
            bytes,
            mode: mode.into(),
        })
    }
}
impl HandoffHosting for Github {
    fn mark_ready(&mut self, b: &Binding) -> Result<()> {
        let pr = self.pull(b)?;
        ensure!(
            string(&pr["head"], "sha")? == b.head_sha
                && string(&pr["base"], "sha")? == b.base_sha
                && string(&pr["head"], "ref")? == b.head_branch
                && string(&pr["base"], "ref")? == b.base_branch
                && string(&pr["head"]["repo"], "full_name")? == b.source_repository,
            "hosting-ready-pr-changed: refresh preparation"
        );
        let result = self.api("graphql", Some(json!({"query":"mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){pullRequest{id isDraft}}}","variables":{"id":string(&pr,"node_id")?}})))?;
        ensure!(
            result.get("errors").is_none()
                && result["data"]["markPullRequestReadyForReview"]["pullRequest"]["id"]
                    == pr["node_id"]
                && result["data"]["markPullRequestReadyForReview"]["pullRequest"]["isDraft"]
                    == false,
            "hosting-ready-failed: ready conversion not confirmed"
        );
        Ok(())
    }
    fn authenticated_login(&mut self) -> Result<String> {
        Ok(string(&self.api("user", None)?, "login")?.into())
    }
    fn list_comments(&mut self, b: &Binding, cursor: Option<&str>) -> Result<CommentPage> {
        let page = cursor
            .unwrap_or("1")
            .parse::<u32>()
            .context("hosting-comment-cursor")?;
        ensure!((1..=10000).contains(&page), "hosting-comment-page-limit");
        let values = self.api(
            &format!(
                "repos/{}/issues/{}/comments?per_page=100&page={page}",
                b.repository, b.pr_number
            ),
            None,
        )?;
        let values = values.as_array().context("hosting-comment-page-invalid")?;
        ensure!(values.len() <= 100, "hosting-comment-page-oversize");
        Ok(CommentPage {
            comments: values
                .iter()
                .map(|v| comment(v, b))
                .collect::<Result<_>>()?,
            next: (values.len() == 100).then(|| (page + 1).to_string()),
        })
    }
    fn read_comment(&mut self, b: &Binding, id: u64) -> Result<Comment> {
        comment(
            &self.api(
                &format!("repos/{}/issues/comments/{id}", b.repository),
                None,
            )?,
            b,
        )
    }
    fn create_comment(&mut self, b: &Binding, body: &str) -> Result<Comment> {
        comment(
            &self.api(
                &format!("repos/{}/issues/{}/comments", b.repository, b.pr_number),
                Some(json!({"body":body})),
            )?,
            b,
        )
    }
}
