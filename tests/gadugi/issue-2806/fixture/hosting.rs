//! Stateful local hosting fixture. Reads never advance pending checks.
use super::environment::{confined_path, observe_git};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checks {
    Pending,
    Green,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub state: String,
    pub branch: String,
    pub base: String,
    pub head_sha: String,
    pub title: String,
    pub body: String,
    pub checks: Checks,
    pub comments: Vec<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    pub cwd: PathBuf,
    pub args: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
struct State {
    repository: String,
    origin: PathBuf,
    prs: Vec<PullRequest>,
}
pub struct HostingStore {
    path: PathBuf,
    state: State,
}
pub struct Response {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl Response {
    fn json(value: Value) -> Result<Self> {
        Ok(Self {
            exit_code: 0,
            stdout: serde_json::to_vec(&value)?,
            stderr: vec![],
        })
    }
    fn text(value: &str) -> Self {
        Self {
            exit_code: 0,
            stdout: format!("{value}\n").into_bytes(),
            stderr: vec![],
        }
    }
    fn refused(reason: &str) -> Self {
        Self {
            exit_code: 1,
            stdout: vec![],
            stderr: format!("{reason}\n").into_bytes(),
        }
    }
}
impl HostingStore {
    pub fn create(path: &Path, repository: &str, origin: &Path) -> Result<Self> {
        ensure!(
            origin.is_absolute() && origin.is_dir(),
            "hosting origin must be an existing local bare repository"
        );
        ensure!(!path.exists(), "hosting state already exists");
        let store = Self {
            path: path.into(),
            state: State {
                repository: repository.into(),
                origin: origin.canonicalize()?,
                prs: vec![],
            },
        };
        store.persist()?;
        Ok(store)
    }
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.into(),
            state: serde_json::from_slice(&fs::read(path)?)?,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    fn persist(&self) -> Result<()> {
        let next = self.path.with_extension("next");
        fs::write(&next, serde_json::to_vec_pretty(&self.state)?)?;
        fs::rename(next, &self.path)?;
        Ok(())
    }
    pub fn open_prs(&self) -> Result<Vec<PullRequest>> {
        Ok(self
            .state
            .prs
            .iter()
            .filter(|pr| pr.state == "OPEN")
            .cloned()
            .collect())
    }
    pub fn pr(&self, number: u64) -> Result<PullRequest> {
        self.state
            .prs
            .iter()
            .find(|pr| pr.number == number)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown PR {number}"))
    }
    pub fn set_checks(&mut self, number: u64, checks: Checks) -> Result<()> {
        let pr = self
            .state
            .prs
            .iter_mut()
            .find(|pr| pr.number == number)
            .ok_or_else(|| anyhow::anyhow!("unknown PR {number}"))?;
        pr.checks = checks;
        self.persist()
    }
    pub fn invocations(&self) -> Result<Vec<Invocation>> {
        let path = self.path.with_extension("calls.jsonl");
        match fs::read_to_string(path) {
            Ok(data) => data
                .lines()
                .map(|line| Ok(serde_json::from_str(line)?))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(error) => Err(error.into()),
        }
    }
    fn origin_head(&self, branch: &str) -> Result<String> {
        observe_git(
            &self.state.origin,
            &["check-ref-format", "--branch", branch],
        )?;
        observe_git(
            &self.state.origin,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        )
    }
    fn pr_json(&self, pr: &PullRequest) -> Result<Value> {
        let base_sha = self.origin_head(&pr.base)?;
        let conclusion = match pr.checks {
            Checks::Green => "SUCCESS",
            Checks::Failed => "FAILURE",
            Checks::Pending => "",
        };
        let state = match pr.checks {
            Checks::Green => "SUCCESS",
            Checks::Failed => "FAILURE",
            Checks::Pending => "PENDING",
        };
        Ok(
            json!({"number":pr.number,"url":format!("https://github.com/{}/pull/{}",self.state.repository,pr.number),
            "state":pr.state,"headRefName":pr.branch,"headRefOid":pr.head_sha,"baseRefName":pr.base,"baseRefOid":base_sha,
            "title":pr.title,"body":pr.body,"isDraft":false,"isCrossRepository":false,
            "headRepository":{"nameWithOwner":self.state.repository},"headRepositoryOwner":{"login":"fixture"},
            "baseRepository":{"nameWithOwner":self.state.repository},"author":{"login":"fixture"},
            "mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","reviewDecision":"APPROVED",
            "createdAt":"2026-11-18T00:00:00Z","mergedAt":if pr.state == "MERGED" { json!("2026-11-18T00:01:00Z") } else { Value::Null },
            "closedAt":Value::Null,"comments":pr.comments,"reviews":[],
            "statusCheckRollup":[{"name":"fixture-verification","status":if pr.checks == Checks::Pending {"IN_PROGRESS"} else {"COMPLETED"},"conclusion":conclusion,"state":state}]}),
        )
    }
    fn read_body(&self, cwd: &Path, args: &[&str]) -> Result<Option<String>> {
        if let Some(body) = option(args, "--body") {
            return Ok(Some(body.into()));
        }
        let Some(file) = option(args, "--body-file") else {
            return Ok(None);
        };
        ensure!(
            file != "-",
            "stdin body must be supplied by executable adapter"
        );
        let root = self
            .path
            .parent()
            .and_then(Path::parent)
            .context("hosting state has no case root")?;
        let path = cwd.join(file);
        let relative = path
            .strip_prefix(root)
            .context("hosting body file escapes private root")?;
        Ok(Some(fs::read_to_string(confined_path(root, relative)?)?))
    }
    pub fn invoke(&mut self, cwd: &Path, args: &[&str]) -> Result<Response> {
        let invocation = Invocation {
            cwd: cwd.into(),
            args: args.iter().map(|x| (*x).into()).collect(),
        };
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.with_extension("calls.jsonl"))?;
        serde_json::to_writer(&mut log, &invocation)?;
        writeln!(log)?;
        if let Some(repo) = option(args, "--repo").or_else(|| option(args, "-R")) {
            ensure!(
                repo == self.state.repository,
                "wrong hosting repository: {repo}"
            );
        }
        let response = match args {
            ["auth", "status", ..] => Response::text("fixture: authenticated locally"),
            ["api", "user", ..] => Response::json(json!({"login":"fixture"}))?,
            ["api", "graphql", ..] => Response::json(
                json!({"data":{"viewer":{"login":"fixture"},"repository":{"nameWithOwner":self.state.repository,"viewerPermission":"ADMIN"}}}),
            )?,
            ["api", endpoint, ..]
                if endpoint.starts_with(&format!("repos/{}/", self.state.repository)) =>
            {
                if endpoint.ends_with("/comments") {
                    let number = endpoint
                        .split('/')
                        .rev()
                        .nth(1)
                        .context("comment PR missing")?
                        .parse()?;
                    Response::json(json!(self.pr(number)?.comments))?
                } else if endpoint.ends_with("/protection") {
                    Response::json(
                        json!({"required_status_checks":{"strict":true,"contexts":["fixture-verification"]},"required_pull_request_reviews":{"required_approving_review_count":0}}),
                    )?
                } else if endpoint.contains("/rules/branches/") || endpoint.ends_with("/reviews") {
                    Response::json(json!([]))?
                } else if endpoint.contains("/check-runs") {
                    let pr = self.state.prs.first().context("check run has no PR")?;
                    let green = pr.checks == Checks::Green;
                    Response::json(
                        json!({"total_count":1,"check_runs":[{"name":"fixture-verification","head_sha":pr.head_sha,"status":if green {"completed"} else {"in_progress"},"conclusion":if green {json!("success")} else {Value::Null},"app":{"slug":"fixture"}}]}),
                    )?
                } else if endpoint.contains("/actions/runs") {
                    Response::json(json!({"total_count":0,"workflow_runs":[]}))?
                } else {
                    bail!("unsupported hosting endpoint {endpoint}");
                }
            }
            ["repo", "view", ..] => Response::json(
                json!({"nameWithOwner":self.state.repository,"defaultBranchRef":{"name":"main"},"viewerPermission":"ADMIN","url":format!("https://github.com/{}",self.state.repository)}),
            )?,
            ["issue", "view", ..] => Response::json(
                json!({"number":7,"state":"OPEN","title":"Implement answer","body":"Write answer.txt containing 42 and verify it.","url":format!("https://github.com/{}/issues/7",self.state.repository),"comments":[],"closedByPullRequestsReferences":[]}),
            )?,
            ["pr", "create", ..] => {
                let branch = option(args, "--head")
                    .ok_or_else(|| anyhow::anyhow!("pr create requires --head"))?;
                let base = option(args, "--base")
                    .ok_or_else(|| anyhow::anyhow!("pr create requires --base"))?;
                let head = self.origin_head(branch)?;
                self.origin_head(base)?;
                ensure!(
                    !self
                        .state
                        .prs
                        .iter()
                        .any(|pr| pr.state == "OPEN" && pr.branch == branch),
                    "duplicate open PR"
                );
                let number = self.state.prs.len() as u64 + 1;
                let body = self.read_body(cwd, args)?.unwrap_or_default();
                self.state.prs.push(PullRequest {
                    number,
                    state: "OPEN".into(),
                    branch: branch.into(),
                    base: base.into(),
                    head_sha: head,
                    title: option(args, "--title").unwrap_or("Implement answer").into(),
                    body,
                    checks: Checks::Pending,
                    comments: vec![],
                });
                self.persist()?;
                Response::text(&format!(
                    "https://github.com/{}/pull/{number}",
                    self.state.repository
                ))
            }
            ["pr", "list", ..] => {
                let mut prs = Vec::new();
                for pr in &self.state.prs {
                    if option(args, "--head").is_some_and(|branch| branch != pr.branch)
                        || option(args, "--base").is_some_and(|base| base != pr.base)
                    {
                        continue;
                    }
                    let state = option(args, "--state").unwrap_or("open");
                    if state != "all" && !pr.state.eq_ignore_ascii_case(state) {
                        continue;
                    }
                    prs.push(self.pr_json(pr)?);
                }
                Response::json(json!(prs))?
            }
            [
                "pr",
                operation @ ("view" | "checks" | "merge" | "comment" | "edit" | "review" | "ready"),
                ..,
            ] => {
                let identity = args.get(2).filter(|arg| !arg.starts_with('-')).copied();
                let branch = observe_git(cwd, &["branch", "--show-current"])?;
                let found = self.state.prs.iter().position(|pr| match identity {
                    Some(value) => {
                        value == pr.number.to_string()
                            || value == pr.branch
                            || value
                                == format!(
                                    "https://github.com/{}/pull/{}",
                                    self.state.repository, pr.number
                                )
                    }
                    None => pr.branch == branch,
                });
                let Some(index) = found else {
                    return Ok(Response::refused("no matching PR"));
                };
                let pr = self.state.prs[index].clone();
                match *operation {
                    "view" => Response::json(self.pr_json(&pr)?)?,
                    "checks" => {
                        let state = match pr.checks {
                            Checks::Pending => "PENDING",
                            Checks::Green => "SUCCESS",
                            Checks::Failed => "FAILURE",
                        };
                        let bucket = match pr.checks {
                            Checks::Pending => "pending",
                            Checks::Green => "pass",
                            Checks::Failed => "fail",
                        };
                        let mut response = if option(args, "--json").is_some() {
                            Response::json(
                                json!([{"name":"fixture-verification","state":state,"bucket":bucket}]),
                            )?
                        } else {
                            Response::text(&format!("fixture-verification\t{bucket}\t0\tfixture"))
                        };
                        response.exit_code = if pr.checks == Checks::Green { 0 } else { 8 };
                        response
                    }
                    "merge" => {
                        let expected = option(args, "--match-head-commit");
                        let actual = self.origin_head(&pr.branch)?;
                        if pr.state != "OPEN"
                            || pr.checks != Checks::Green
                            || expected != Some(pr.head_sha.as_str())
                            || actual != pr.head_sha
                        {
                            Response::refused(
                                "merge requires OPEN, explicitly green checks and matching requested/PR/origin heads",
                            )
                        } else {
                            self.state.prs[index].state = "MERGED".into();
                            self.persist()?;
                            Response::text("merged")
                        }
                    }
                    "comment" | "review" => {
                        let body = self
                            .read_body(cwd, args)?
                            .context("comment requires body")?;
                        self.state.prs[index].comments.push(json!({"body":body,"author":{"login":"fixture"},"createdAt":"2026-11-18T00:00:00Z"}));
                        self.persist()?;
                        Response::text("comment recorded")
                    }
                    "ready" => Response::text("ready"),
                    "edit" => {
                        if let Some(body) = self.read_body(cwd, args)? {
                            self.state.prs[index].body = body;
                        }
                        self.persist()?;
                        Response::text("updated")
                    }
                    _ => unreachable!(),
                }
            }
            _ => bail!("unsupported hosting invocation: {args:?}"),
        };
        if let Some(filter) = option(args, "--jq").or_else(|| option(args, "-q")) {
            let mut child = Command::new("/usr/bin/jq")
                .env_clear()
                .args(["-r", filter])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("jq stdin missing"))?
                .write_all(&response.stdout)?;
            let output = child.wait_with_output()?;
            ensure!(
                output.status.success(),
                "hosting jq filter failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(Response {
                stdout: output.stdout,
                ..response
            })
        } else {
            Ok(response)
        }
    }
}
pub fn option<'a>(args: &'a [&str], key: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == key)
        .map(|pair| pair[1])
}
