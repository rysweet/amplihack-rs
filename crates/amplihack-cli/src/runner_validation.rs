//! Read-only capability and Cargo receipt validation shared by delivery paths.
use anyhow::{Result, anyhow};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, path::Path};

fn error(code: &str) -> anyhow::Error {
    anyhow!("{code}")
}

// Reject ambiguous top-level properties rather than accepting last-value wins.
struct Report(BTreeMap<String, Value>);
impl<'de> Deserialize<'de> for Report {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Report;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unique report properties")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Report, M::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate report property"));
                    }
                }
                Ok(Report(fields))
            }
        }
        d.deserialize_map(Fields)
    }
}

pub(crate) fn probe(binary: &Path, provider: &str) -> Result<Value> {
    let output = crate::runner_probe::capture(binary).map_err(|_| error("RUNNER_PROBE_FAILED"))?;
    if !output.status.success() {
        return Err(error("RUNNER_PROBE_FAILED"));
    }
    let Report(fields) =
        serde_json::from_slice(&output.stdout).map_err(|_| error("RUNNER_REPORT_INVALID"))?;
    let report = serde_json::to_value(fields)?;
    if report["schema_version"].as_u64() != Some(1)
        || !report["version"]
            .as_str()
            .is_some_and(|v| !v.trim().is_empty())
        || !report["capabilities"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(error("RUNNER_REPORT_INVALID"));
    }
    if provider == "codex"
        && !report["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "codex_exec")
    {
        return Err(error("RUNNER_CAPABILITY_MISSING"));
    }
    Ok(report)
}

pub(crate) fn provenance(
    binary: &Path,
    cargo_home: &Path,
    repository: &str,
    revision: &str,
) -> Result<()> {
    let selected = binary
        .canonicalize()
        .map_err(|_| error("RUNNER_BINARY_SHADOWED"))?;
    let managed = cargo_home
        .join("bin/recipe-runner-rs")
        .canonicalize()
        .map_err(|_| error("RUNNER_BINARY_SHADOWED"))?;
    if selected != managed {
        return Err(error("RUNNER_BINARY_SHADOWED"));
    }
    let bytes = std::fs::read(cargo_home.join(".crates2.json"))
        .map_err(|_| error("RUNNER_RECEIPT_MISSING"))?;
    let receipt: Value =
        serde_json::from_slice(&bytes).map_err(|_| error("RUNNER_PROVENANCE_MISMATCH"))?;
    let valid = receipt["installs"].as_object().is_some_and(|entries| {
        entries.iter().any(|(source, record)| {
            let Some(rest) = source.strip_prefix("recipe-runner-rs ") else {
                return false;
            };
            let Some((version, origin)) = rest.split_once(" (git+") else {
                return false;
            };
            let Some(origin) = origin.strip_suffix(')') else {
                return false;
            };
            let Some((url, sha)) = origin.rsplit_once('#') else {
                return false;
            };
            !version.is_empty()
                && url.split('?').next() == Some(repository)
                && sha == revision
                && record["bins"]
                    .as_array()
                    .is_some_and(|bins| bins.iter().any(|b| b == "recipe-runner-rs"))
        })
    });
    if !valid {
        return Err(error("RUNNER_PROVENANCE_MISMATCH"));
    }
    Ok(())
}

#[derive(clap::Subcommand, Debug)]
pub enum InternalCommands {
    ValidateRecipeRunner(ValidationArgs),
}
#[derive(clap::Args, Debug)]
pub struct ValidationArgs {
    #[arg(long, value_parser = ["claude", "copilot", "codex"])]
    provider: String,
    #[arg(long)]
    binary: std::path::PathBuf,
    #[arg(long)]
    cargo_home: Option<std::path::PathBuf>,
    #[arg(long)]
    repository: Option<String>,
    #[arg(long)]
    revision: Option<String>,
}

pub(crate) fn run(command: InternalCommands) -> Result<()> {
    let InternalCommands::ValidateRecipeRunner(args) = command;
    let result = (|| -> Result<Value> {
        if !args.binary.is_absolute() {
            return Err(error("INVALID_VALIDATION_REQUEST"));
        }
        let managed = match (&args.cargo_home, &args.repository, &args.revision) {
            (None, None, None) => false,
            (Some(home), Some(repo), Some(rev))
                if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                provenance(&args.binary, home, repo, rev)?;
                true
            }
            _ => return Err(error("INVALID_VALIDATION_REQUEST")),
        };
        let runner = probe(&args.binary, &args.provider)?;
        let provenance = if managed {
            json!({"kind":"managed", "package":"recipe-runner-rs", "repository":args.repository,
                "revision":args.revision, "cargo_home":args.cargo_home})
        } else {
            json!({"kind":"explicit"})
        };
        Ok(
            json!({"schema_version":1,"provider":args.provider,"runner":runner,
            "binary":args.binary.canonicalize().map_err(|_| error("RUNNER_PROBE_FAILED"))?, "provenance":provenance}),
        )
    })();
    match result {
        Ok(value) => {
            println!("{value}");
            Ok(())
        }
        Err(error) => {
            let code = error.to_string();
            let message = match code.as_str() {
                "RUNNER_CAPABILITY_MISSING" => {
                    "runner must advertise codex_exec; fix or unset the override"
                }
                "RUNNER_REPORT_INVALID" => {
                    "runner must emit schema 1, a nonblank version and string capabilities"
                }
                "RUNNER_PROBE_FAILED" => {
                    "runner capability probe failed; check executable compatibility and permissions"
                }
                "RUNNER_RECEIPT_MISSING" => "Cargo receipt missing; reinstall the managed runner",
                "RUNNER_BINARY_SHADOWED" => {
                    "selected runner differs from the managed Cargo binary; fix PATH"
                }
                "RUNNER_PROVENANCE_MISMATCH" => {
                    "Cargo receipt does not bind package, repository, revision and binary; reinstall"
                }
                _ => {
                    "provide an absolute binary and all or none of cargo-home, repository and revision"
                }
            };
            eprintln!(
                "{}",
                json!({"error":{"code":code,"message":message,"details":{"provider":args.provider}}})
            );
            std::process::exit(1)
        }
    }
}
