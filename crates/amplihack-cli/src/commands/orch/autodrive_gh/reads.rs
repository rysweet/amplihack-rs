//! The reads of `autodrive_gh`: each source answer mapped to the GraphQL
//! spelling, then the cascade per read (issue #2333).

use super::{
    Gh, MAX_PAGES, Read, Trail, excerpt, ls_remote, now, origin_is, ratelimit, rest_map,
    valid_branch, valid_sha,
};
use serde_json::{Map, Value, json};

// ── mapping answers ─────────────────────────────────────────────────────────

fn sha_of(v: &Value) -> Result<String, String> {
    v.as_str()
        .filter(|s| valid_sha(s))
        .map(str::to_string)
        .ok_or_else(|| "answer has no valid head SHA".to_string())
}

fn branch_of(v: &Value, what: &str) -> Result<String, String> {
    v.as_str()
        .filter(|s| valid_branch(s))
        .map(str::to_string)
        .ok_or_else(|| format!("answer has no valid {what}"))
}

fn merged_at(v: &Value) -> Value {
    match v.as_str() {
        Some(s) if !s.is_empty() && !s.starts_with("0001-") => json!(s),
        _ => Value::Null,
    }
}

fn pr_from_graphql(v: &Value) -> Result<Map<String, Value>, String> {
    let state = v["state"]
        .as_str()
        .and_then(rest_map::graphql_state)
        .ok_or("answer has no valid state")?;
    Ok(fields(json!({
        "state": state,
        "is_draft": v["isDraft"].as_bool().unwrap_or(false),
        "head_sha": sha_of(&v["headRefOid"])?,
        "head_ref": branch_of(&v["headRefName"], "head ref")?,
        "base_ref": branch_of(&v["baseRefName"], "base ref")?,
        "merge_state": rest_map::graphql_merge_state(v["mergeStateStatus"].as_str().unwrap_or("")),
        "mergeable": rest_map::graphql_mergeable(v["mergeable"].as_str().unwrap_or("")),
        "merged_at": merged_at(&v["mergedAt"]),
    })))
}

fn pr_from_rest(v: &Value) -> Result<Map<String, Value>, String> {
    let state = v["state"].as_str().ok_or("answer has no state")?;
    Ok(fields(json!({
        "state": rest_map::pr_state(state, &merged_at(&v["merged_at"])),
        "is_draft": v["draft"].as_bool().unwrap_or(false),
        "head_sha": sha_of(&v["head"]["sha"])?,
        "head_ref": branch_of(&v["head"]["ref"], "head ref")?,
        "base_ref": branch_of(&v["base"]["ref"], "base ref")?,
        "merge_state": rest_map::merge_state(v["mergeable_state"].as_str().unwrap_or("")),
        "mergeable": rest_map::mergeable(&v["mergeable"]),
        "merged_at": merged_at(&v["merged_at"]),
    })))
}

fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn head_fields(sha: Value, head_ref: &Value) -> Result<Map<String, Value>, String> {
    let mut m = Map::new();
    m.insert("head_sha".into(), json!(sha_of(&sha)?));
    if let Ok(r) = branch_of(head_ref, "head ref") {
        m.insert("head_ref".into(), json!(r));
    }
    Ok(m)
}

// ── the reads ───────────────────────────────────────────────────────────────

type Answer = Result<Map<String, Value>, String>;
type Source<'a> = (&'static str, Box<dyn FnOnce() -> Answer + 'a>);

fn src<'a>(name: &'static str, f: impl FnOnce() -> Answer + 'a) -> Source<'a> {
    (name, Box::new(f))
}

/// Tries each source in order; the first that answers wins.
fn cascade(gh: &Gh, sources: Vec<Source<'_>>) -> Read {
    let mut trail = Trail::default();
    for (name, f) in sources {
        match f() {
            Ok(fields) => {
                return Read::Answer {
                    source: name,
                    fields,
                };
            }
            Err(e) => trail.fail(name, &e),
        }
    }
    Read::Unavailable(trail.unavailable(gh))
}

/// State, draft, head, base, merge state, mergeable and merged time.
pub fn pr(gh: &Gh, n: u64) -> Read {
    cascade(
        gh,
        vec![
            src("graphql", move || {
                pr_from_graphql(&gh.view(
                        n,
                        "number,state,isDraft,headRefOid,headRefName,baseRefName,mergeStateStatus,mergeable,mergedAt",
                    )?)
            }),
            src("rest", move || {
                pr_from_rest(&gh.rest(&format!("pulls/{n}"))?)
            }),
        ],
    )
}

/// The PR head SHA: GraphQL, REST, then `git ls-remote`.
pub fn head(gh: &Gh, n: u64) -> Read {
    cascade(
        gh,
        vec![
            src("graphql", move || {
                // `--jq` prints "<sha> <ref>": the shape the dispatch read before #2333.
                let mut a: Vec<String> = vec![
                    "pr".into(),
                    "view".into(),
                    n.to_string(),
                    "--json".into(),
                    "headRefOid,headRefName".into(),
                    "--jq".into(),
                    ".headRefOid + \" \" + (.headRefName // \"\")".into(),
                ];
                a.extend(gh.repo_flag());
                let o = gh.run(&a);
                if !o.ok {
                    return Err(o.err);
                }
                let text = String::from_utf8_lossy(&o.stdout);
                let mut words = text.split_whitespace();
                let sha = words.next().unwrap_or_default().to_string();
                head_fields(json!(sha), &json!(words.next()))
            }),
            src("rest", move || {
                let v = gh.rest(&format!("pulls/{n}"))?;
                head_fields(v["head"]["sha"].clone(), &v["head"]["ref"])
            }),
            src("ls-remote", move || {
                // IV2: `git ls-remote origin` reads origin, whatever --repo says.
                if let Some(repo) = gh.repo {
                    origin_is(gh.dir, repo)?;
                }
                head_fields(json!(ls_remote(gh.dir, n)?), &Value::Null)
            }),
        ],
    )
}

fn rest_checks(gh: &Gh, sha: &str) -> Result<Map<String, Value>, String> {
    let runs = gh.rest_pages(&format!("commits/{sha}/check-runs"), Some("check_runs"))?;
    let statuses = gh.rest_pages(&format!("commits/{sha}/status"), Some("statuses"))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut checks = Vec::new();
    let mut push = |name: Option<&str>, bucket: &str| {
        if let Some(name) = name
            && seen.insert(name.to_string())
        {
            checks.push(json!({"name": name, "bucket": bucket}));
        }
    };
    for r in &runs {
        let b = rest_map::check_run_bucket(
            r["status"].as_str().unwrap_or(""),
            r["conclusion"].as_str(),
        );
        push(r["name"].as_str(), b);
    }
    for s in &statuses {
        push(
            s["context"].as_str(),
            rest_map::status_bucket(s["state"].as_str().unwrap_or("")),
        );
    }
    Ok(fields(json!({"head_sha": sha, "checks": checks})))
}

/// What `checks` reads: a PR's head, or one commit.
#[derive(Debug, Clone)]
pub enum Target {
    Pr(u64),
    Sha(String),
}

/// The check buckets at the head of a PR, or at one commit (REST only).
pub fn checks(gh: &Gh, target: &Target) -> Read {
    match target {
        Target::Sha(sha) => {
            let sha = sha.clone();
            cascade(gh, vec![src("rest", move || rest_checks(gh, &sha))])
        }
        Target::Pr(n) => {
            let n = *n;
            cascade(
                gh,
                vec![
                    src("graphql", move || {
                        let head = sha_of(&gh.view(n, "headRefOid")?["headRefOid"])?;
                        let mut a = vec![
                            "pr".into(),
                            "checks".into(),
                            n.to_string(),
                            "--json".into(),
                            "name,bucket".into(),
                        ];
                        a.extend(gh.repo_flag());
                        // gh exits 8 while checks are pending and 1 when one
                        // failed, with the list on stdout either way.
                        let o = gh.run(&a);
                        let list: Vec<Value> = serde_json::from_slice(&o.stdout).map_err(|_| {
                            if o.ok {
                                "answer is not a list".into()
                            } else {
                                o.err.clone()
                            }
                        })?;
                        let checks: Vec<Value> = list
                                .iter()
                                .filter_map(|c| {
                                    Some(json!({
                                        "name": c["name"].as_str()?,
                                        "bucket": rest_map::graphql_bucket(c["bucket"].as_str().unwrap_or("")),
                                    }))
                                })
                                .collect();
                        Ok(fields(json!({"head_sha": head, "checks": checks})))
                    }),
                    src("rest", move || {
                        let head = sha_of(&gh.rest(&format!("pulls/{n}"))?["head"]["sha"])?;
                        rest_checks(gh, &head)
                    }),
                ],
            )
        }
    }
}

const THREADS_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor} nodes{isResolved}}}}}";

/// Unresolved review threads. GraphQL only: there is no REST path.
fn unresolved_threads(gh: &Gh, n: u64) -> Result<u64, String> {
    let (owner, name) = gh.owner_value();
    let flag = gh.field_flag();
    let mut after: Option<String> = None;
    let mut open = 0;
    for _ in 0..MAX_PAGES {
        let mut a: Vec<String> = vec![
            "api".into(),
            "graphql".into(),
            "-f".into(),
            format!("query={THREADS_QUERY}"),
            flag.into(),
            format!("owner={owner}"),
            flag.into(),
            format!("name={name}"),
            "-F".into(),
            format!("number={n}"),
        ];
        if let Some(c) = &after {
            a.extend(["-f".into(), format!("after={c}")]);
        }
        let v = gh.json(&a)?;
        let t = &v["data"]["repository"]["pullRequest"]["reviewThreads"];
        let nodes = t["nodes"].as_array().ok_or("answer holds no threads")?;
        open += nodes
            .iter()
            .filter(|x| x["isResolved"] == json!(false))
            .count() as u64;
        if t["pageInfo"]["hasNextPage"] != json!(true) {
            return Ok(open);
        }
        after = t["pageInfo"]["endCursor"].as_str().map(str::to_string);
        if after.is_none() {
            return Err("answer has no page cursor".into());
        }
    }
    Err(format!("answer over {MAX_PAGES} pages"))
}

/// Review decision, approvals, changes requested and unresolved threads.
pub fn reviews(gh: &Gh, n: u64, required: u64) -> Read {
    let mut read = cascade(
        gh,
        vec![
            src("graphql", move || {
                let v = gh.view(n, "reviewDecision,latestReviews")?;
                let latest = v["latestReviews"].as_array().cloned().unwrap_or_default();
                let (approvals, changes) = rest_map::review_counts(&latest);
                let decision = match v["reviewDecision"].as_str() {
                    Some(d @ ("APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED")) => d,
                    _ => rest_map::review_decision(&latest, required),
                };
                Ok(fields(
                    json!({"review_decision": decision, "approvals": approvals,
                                      "changes_requested": changes}),
                ))
            }),
            src("rest", move || {
                let list = gh.rest_pages(&format!("pulls/{n}/reviews"), None)?;
                let (approvals, changes) = rest_map::review_counts(&list);
                Ok(fields(
                    json!({"review_decision": rest_map::review_decision(&list, required),
                                      "approvals": approvals, "changes_requested": changes}),
                ))
            }),
        ],
    );
    if let Read::Answer { fields, .. } = &mut read {
        let threads = match unresolved_threads(gh, n) {
            Ok(k) => json!(k),
            Err(_) => json!("unavailable"),
        };
        fields.insert("unresolved_threads".into(), threads);
    }
    read
}

fn first_number(list: &Value) -> Result<Map<String, Value>, String> {
    let list = list.as_array().ok_or("answer is not a list")?;
    let number = match list.first() {
        None => Value::Null,
        Some(p) => json!(
            p["number"]
                .as_u64()
                .filter(|k| (1..=9_999_999_999).contains(k))
                .ok_or("answer has no valid PR number")?
        ),
    };
    Ok(fields(json!({ "number": number })))
}

/// The open PR whose head is `branch`; `number` is null when there is none.
pub fn pr_for_branch(gh: &Gh, branch: &str) -> Read {
    let branch = branch.to_string();
    let b2 = branch.clone();
    cascade(
        gh,
        vec![
            // `gh pr view <branch>` is how gh itself finds a branch's PR. A
            // branch of digits would read as a PR number, so REST answers it.
            src("graphql", move || {
                if branch.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("a branch of digits is read through REST".into());
                }
                let mut a: Vec<String> = vec![
                    "pr".into(),
                    "view".into(),
                    branch.clone(),
                    "--json".into(),
                    "number,state".into(),
                ];
                a.extend(gh.repo_flag());
                let v = gh.json(&a)?;
                if v["state"].as_str() != Some("OPEN") {
                    return Err("the branch's PR is not open".into());
                }
                first_number(&json!([v]))
            }),
            src("rest", move || {
                let (owner, _) = gh.owner_value();
                first_number(&gh.json(&[
                    "api".into(),
                    "-X".into(),
                    "GET".into(),
                    gh.endpoint("pulls"),
                    gh.field_flag().into(),
                    format!("head={owner}:{b2}"),
                    "-f".into(),
                    "state=open".into(),
                ])?)
            }),
        ],
    )
}

/// `PUT …/pulls/{n}/update-branch` with `expected_head_sha`, once (R7). The
/// outcome is always recorded; it never merges and is never retried.
pub fn update_branch(gh: &Gh, n: u64, expected: &str) -> Value {
    let o = gh.run(&[
        "api".into(),
        "-X".into(),
        "PUT".into(),
        gh.endpoint(&format!("pulls/{n}/update-branch")),
        "-f".into(),
        format!("expected_head_sha={expected}"),
    ]);
    let mut v =
        json!({"pr": n, "action": "update-branch", "result": "ok", "expected_head_sha": expected});
    if o.ok {
        return v;
    }
    let lower = o.err.to_lowercase();
    if lower.contains("422") && lower.contains("no new commits") {
        v["action"] = json!("noop-up-to-date");
        return v;
    }
    match ratelimit::classify(&o.err) {
        c @ (ratelimit::Class::RateLimit | ratelimit::Class::Transient) => {
            let limited = c == ratelimit::Class::RateLimit;
            let now = now();
            let reset = if limited {
                ratelimit::reset_from_text(&o.err, now).or_else(|| gh.reset_lookup(false, now))
            } else {
                None
            };
            v["result"] = json!("unavailable");
            v["rate_limited"] = json!(limited);
            v["reset_epoch"] = json!(reset);
        }
        _ => v["result"] = json!("refused"),
    }
    v["reason"] = json!(excerpt(&o.err));
    v
}

/// The head of the current branch's PR through GraphQL, as the completion
/// dispatch read it before #2333 (`gh pr view` with no number). `None` when
/// that read fails or answers no valid SHA; the caller then resolves the PR
/// number and runs the full cascade.
pub fn current_branch_head(gh: &Gh) -> Option<String> {
    let mut a: Vec<String> = ["pr", "view", "--json", "headRefOid", "--jq", ".headRefOid"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(gh.repo_flag());
    let o = gh.run(&a);
    let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.ok && valid_sha(&text)).then_some(text)
}
