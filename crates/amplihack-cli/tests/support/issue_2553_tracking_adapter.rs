//! Production `gh` command boundary, backed only by private fixture files.

use serde_json::{Value, json};
use std::{env, fs};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mode = env::var("ISSUE_2553_TRACKING_MODE").unwrap_or_else(|_| "success".into());
    let root = super::root();
    if args.starts_with(&["issue".into(), "create".into()]) {
        let record = root.join("created-issue.json");
        anyhow::ensure!(!record.exists(), "duplicate tracking acquisition");
        if mode == "failure" {
            anyhow::bail!("fixture tracking service refused acquisition: fixture-secret-token");
        }
        let body = args
            .windows(2)
            .find(|pair| pair[0] == "--body")
            .map(|pair| pair[1].as_str())
            .unwrap_or("");
        let issue = json!({"number": 711, "url": "https://github.com/fixture/repository/issues/711", "body": body});
        fs::write(record, serde_json::to_vec(&issue)?)?;
        if mode == "uncertain" {
            anyhow::bail!("fixture lost acknowledgement after creating issue");
        }
        match mode.as_str() {
            "zero" => {
                println!("0");
                return Ok(());
            }
            "absent" => {
                println!("TBD");
                return Ok(());
            }
            "untrusted" => {
                println!("https://fixture-secret-token.example/issues/711");
                return Ok(());
            }
            _ => (),
        }
        if args.iter().any(|arg| arg == "--json") {
            println!("{issue}");
        } else {
            println!("https://github.com/fixture/repository/issues/711");
        }
    } else if args.starts_with(&["issue".into(), "list".into()]) {
        let issue: Value = fs::read(root.join("created-issue.json"))
            .ok()
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()?
            .unwrap_or(Value::Null);
        let candidates = match mode.as_str() {
            "fuzzy" => json!([{"number": 712, "body": "approximate match"}, issue]),
            "duplicate" => json!([issue.clone(), issue]),
            "empty" => json!([]),
            "saturated" => {
                let mut candidates = vec![issue];
                candidates
                    .extend((0..99).map(|_| json!({"number": 712, "body": "approximate match"})));
                json!(candidates)
            }
            "wrong-reference" => json!([{"number": 712, "body": "approximate match"}]),
            "invalid-number" => json!([{"number": 0, "body": issue["body"]}]),
            "malformed" => {
                println!("not JSON");
                return Ok(());
            }
            "invalid-json-type" => json!("fixture-secret-token"),
            "failure" => anyhow::bail!("fixture-secret-token"),
            _ if issue.is_null() => json!([]),
            _ => json!([issue]),
        };
        println!("{candidates}");
    } else if args.starts_with(&["pr".into(), "view".into()]) {
        println!("MERGED");
    } else {
        anyhow::bail!("unexpected fixture gh command: {args:?}");
    }
    Ok(())
}
