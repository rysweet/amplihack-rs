//! Pure leaf-plan validation. Only the wrapper resolves trusted repository locations.
use super::identity::WorkstreamId;
use super::issue::parse_optional;
use super::models::WorkstreamConfig;
pub use super::trusted::{private_directory, repositories, repository_names};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tracking {
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_result: Option<Binding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub workstream_id: WorkstreamId,
    pub output: String,
}

pub fn deserialize_lifecycle<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if [
        "pending",
        "tracking",
        "ready",
        "running",
        "succeeded",
        "failed",
        "blocked",
        "reconciliation_required",
        "abandoned",
    ]
    .contains(&s.as_str())
    {
        Ok(s)
    } else {
        Err(serde::de::Error::custom(format!("unknown lifecycle {s:?}")))
    }
}

pub fn classification(raw: &str) -> Result<&'static str> {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .as_str()
    {
        "operations" | "ops" | "pm" | "project management" | "projectmanagement" | "ops/pm"
        | "operations/pm" | "ops workflow" => Ok("Operations"),
        "development" | "dev" | "default" | "default workflow" => Ok("Development"),
        "investigation" | "research" | "investigation workflow" => Ok("Investigation"),
        "q&a" | "qa" | "q&a workflow" => Ok("Q&A"),
        "consensus" | "consensus workflow" => Ok("Consensus"),
        _ => bail!("unrecognised classification {raw:?}"),
    }
}

pub fn route(class: &str, override_value: &str) -> &'static str {
    match class {
        "Operations" => "operations-direct",
        "Investigation" => "investigation-workflow",
        "Q&A" => "qa-workflow",
        "Consensus" => "consensus-workflow",
        _ if override_value == "quick-change" => "quick-change",
        _ => "default-workflow",
    }
}

fn field_class(v: &Value) -> Option<&Value> {
    ["classification", "task_type", "type"]
        .iter()
        .find_map(|key| v.get(*key))
}

fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("{key} must be a non-blank string"))
}

/// The I/O wrapper allocates identities once; normalisation itself is deterministic.
pub fn allocate_identities(input: &mut Value) -> Result<()> {
    let leaves = input
        .get_mut("workstreams")
        .and_then(Value::as_array_mut)
        .context("plan.workstreams must be an array")?;
    for leaf in leaves {
        let object = leaf
            .as_object_mut()
            .context("each workstream must be an object")?;
        if !object.contains_key("workstream_id") {
            object.insert("workstream_id".into(), json!(WorkstreamId::new()));
        }
    }
    Ok(())
}

/// Validate every leaf and dependency before returning any executable configuration.
/// Repository names are authorised by the caller, never by decomposition output.
pub fn normalise_plan(
    input: &Value,
    repositories: &[String],
    override_value: &str,
) -> Result<Vec<WorkstreamConfig>> {
    let leaves = input
        .get("workstreams")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
        .context("plan.workstreams must be a non-empty array")?;
    let top = field_class(input)
        .map(|v| {
            v.as_str()
                .context("plan classification must be a string")
                .and_then(classification)
        })
        .transpose()?;
    let mut keys = HashMap::new();
    let mut ids = HashSet::new();
    let mut explicit = Vec::new();
    for (index, leaf) in leaves.iter().enumerate() {
        let key = leaf
            .get("key")
            .or_else(|| leaf.get("name"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .with_context(|| format!("leaf {index}: name/key must be non-blank"))?;
        ensure!(
            keys.insert(key.to_owned(), index).is_none(),
            "duplicate leaf key {key:?}"
        );
        explicit.push(
            field_class(leaf)
                .map(|v| {
                    v.as_str()
                        .with_context(|| format!("leaf {key}: classification must be a string"))
                        .and_then(classification)
                })
                .transpose()
                .with_context(|| format!("leaf {key}: classification"))?,
        );
    }
    let classes: HashSet<_> = explicit.iter().filter_map(|c| *c).chain(top).collect();
    if classes.len() > 1 {
        ensure!(
            explicit.iter().all(Option::is_some),
            "mixed plans require explicit classifications on every leaf"
        );
    }
    let mut entries = Vec::new();
    for (index, leaf) in leaves.iter().enumerate() {
        let key = leaf
            .get("key")
            .or_else(|| leaf.get("name"))
            .and_then(Value::as_str)
            .unwrap();
        let entry = (|| -> Result<WorkstreamConfig> {
            let task = leaf.get("task").or_else(|| leaf.get("description")).and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty()).context("task/description must be a non-blank string")?;
            let class = explicit[index].or(top).unwrap_or("Development");
            if class == "Operations" {
                ensure!(!amplihack_workflows::classifier::requires_repository_edit(task), "repository edits require a Development leaf");
            }
            let repository = match leaf.get("repository") {
                Some(_) => text(leaf, "repository")?,
                None => repositories.first().map(String::as_str).context("no trusted repository")?,
            };
            ensure!(repositories.iter().any(|r| r == repository), "repository {repository:?} is not authorised");
            let id: WorkstreamId = match leaf.get("workstream_id") {
                Some(v) => serde_json::from_value(v.clone()).context("invalid workstream_id")?,
                None => bail!("workstream_id must be allocated before normalisation"),
            };
            ensure!(ids.insert(id), "duplicate workstream_id {id}");
            let issue = parse_optional(leaf.get("issue").unwrap_or(&Value::Null)).context("issue")?;
            let mut value = leaf.clone();
            value["workstream_id"] = json!(id);
            value["key"] = json!(key);
            value["repository"] = json!(repository);
            value["classification"] = json!(class);
            value["task"] = json!(task);
            value["description"] = json!(key);
            value["issue"] = json!(issue);
            value["depends_on"] = json!([]);
            value["recipe"] = json!(route(class, override_value));
            if class == "Development" || class == "Consensus" {
                let slug: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(30).collect();
                let slug = slug.trim_matches('-');
                let slug = if slug.is_empty() { format!("ws-{}", index + 1) } else { slug.to_owned() };
                value["branch"] = leaf.get("branch").cloned().unwrap_or_else(|| json!(format!("feat/orch-{}-{slug}", index + 1)));
            } else {
                value.as_object_mut().unwrap().remove("branch");
            }
            // Bindings are resolved in the complete-plan pass below.
            if let Some(tracking) = value.get_mut("tracking") {
                if let Some(binding) = tracking.get_mut("from_result") {
                    *binding = json!({"workstream_id": id, "output": binding.get("output").cloned().unwrap_or(Value::Null)});
                }
            }
            if let Some(v) = value.get_mut("is_critical_code") {
                if let Some(b) = v.as_bool() { *v = json!(b.to_string()); }
            }
            let mut entry: WorkstreamConfig = serde_json::from_value(value)?;
            if entry.required_outputs.is_empty() {
                entry.required_outputs.insert(if class == "Development" || class == "Consensus" { "delivery" } else { "answer" }.into(), if class == "Development" || class == "Consensus" { "delivery" } else { "answer" }.into());
            }
            for (name, kind) in &entry.required_outputs {
                ensure!(!name.trim().is_empty() && ["answer", "issue", "operation", "delivery"].contains(&kind.as_str()), "invalid required_outputs entry {name:?}: {kind:?}");
            }
if class == "Development" || class == "Consensus" {
    ensure!(entry.required_outputs.values().all(|kind| kind == "delivery"), "repository editing leaves require delivery outputs");
}
            Ok(entry)
        })().with_context(|| format!("leaf {key}"))?;
        entries.push(entry);
    }
    for (index, leaf) in leaves.iter().enumerate() {
        let mut dependencies = Vec::new();
        if let Some(raw) = leaf.get("depends_on") {
            for dep in raw.as_array().context("depends_on must be an array")? {
                let name = dep
                    .as_str()
                    .context("dependency must be a key or execution UUID")?;
                let other = keys
                    .get(name)
                    .copied()
                    .or_else(|| {
                        entries
                            .iter()
                            .position(|e| e.workstream_id.is_some_and(|id| id.to_string() == name))
                    })
                    .with_context(|| {
                        format!("leaf {}: missing dependency {name}", entries[index].key)
                    })?;
                ensure!(
                    other != index,
                    "leaf {}: self dependency",
                    entries[index].key
                );
                let id = entries[other].workstream_id.unwrap();
                ensure!(
                    !dependencies.contains(&id),
                    "leaf {}: duplicate dependency",
                    entries[index].key
                );
                dependencies.push(id);
            }
        }
        entries[index].depends_on = dependencies;
        if let Some(binding) = leaf.get("tracking").and_then(|t| t.get("from_result")) {
            let producer = binding
                .get("workstream")
                .or_else(|| binding.get("workstream_id"))
                .and_then(Value::as_str)
                .context("tracking.from_result.workstream is required")?;
            let other = keys
                .get(producer)
                .copied()
                .or_else(|| {
                    entries
                        .iter()
                        .position(|e| e.workstream_id.is_some_and(|id| id.to_string() == producer))
                })
                .context("tracking binding has no producer")?;
            let id = entries[other].workstream_id.unwrap();
            let output = text(binding, "output")?;
            ensure!(
                entries[index].depends_on.contains(&id),
                "tracking producer must be a dependency"
            );
            ensure!(
                entries[other].repository == entries[index].repository,
                "tracking repository mismatch"
            );
            ensure!(
                entries[other]
                    .required_outputs
                    .get(output)
                    .is_some_and(|k| k == "issue"),
                "tracking binding must reference a declared issue output"
            );
            ensure!(
                entries[index].issue.is_null()
                    && entries[index].tracking.title.is_none()
                    && entries[index].tracking.body.is_none(),
                "tracking binding conflicts with issue or creation details"
            );
            entries[index].tracking.from_result = Some(Binding {
                workstream_id: id,
                output: output.to_owned(),
            });
        }
        let e = &entries[index];
        if e.tracking.required && e.issue.is_null() && e.tracking.from_result.is_none() {
            ensure!(
                e.tracking
                    .title
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty())
                    && e.tracking
                        .body
                        .as_ref()
                        .is_some_and(|s| !s.trim().is_empty()),
                "leaf {}: required tracking needs non-blank title and body",
                e.key
            );
        }
    }
    validate_dependencies(&entries)?;
    Ok(entries)
}

pub fn validate_dependencies(entries: &[WorkstreamConfig]) -> Result<()> {
    let mut done = HashSet::new();
    loop {
        let before = done.len();
        for e in entries {
            let id = e.workstream_id.context("missing execution identity")?;
            if e.depends_on.iter().all(|id| done.contains(id)) {
                done.insert(id);
            }
        }
        if done.len() == entries.len() {
            return Ok(());
        }
        ensure!(
            done.len() != before,
            "dependency cycle or unknown execution identity"
        );
    }
}
