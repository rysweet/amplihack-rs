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
            anyhow::bail!("fixture tracking service refused acquisition");
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
        println!(
            "{}",
            if issue.is_null() {
                json!([])
            } else {
                json!([issue])
            }
        );
    } else if args.starts_with(&["pr".into(), "view".into()]) {
        println!("MERGED");
    } else {
        anyhow::bail!("unexpected fixture gh command: {args:?}");
    }
    Ok(())
}
