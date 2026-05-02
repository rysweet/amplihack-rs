use chrono::{DateTime, NaiveDateTime, Utc};

use crate::orchestrator::VM;

/// Parse JSON output from `azlin list --json`.
pub(crate) fn parse_azlin_list_json(output: &str) -> Vec<VM> {
    let items: Vec<serde_json::Value> = match serde_json::from_str(output) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    items
        .into_iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let size = item
                .get("size")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let region = item
                .get("region")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let created_at = item
                .get("created_at")
                .and_then(|v| v.as_str())
                .and_then(parse_timestamp);

            Some(VM {
                name,
                size,
                region,
                created_at,
                tags: None,
            })
        })
        .collect()
}

/// Parse text output from `azlin list`.
pub(crate) fn parse_azlin_list_text(output: &str) -> Vec<VM> {
    let mut vms = Vec::new();
    for line in output.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            vms.push(VM {
                name: parts[0].to_string(),
                size: parts[1].to_string(),
                region: parts[2].to_string(),
                created_at: None,
                tags: None,
            });
        }
    }
    vms
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    let formats = ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y%m%d-%H%M%S"];
    for fmt in formats {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_azlin_json() {
        let json = r#"[
            {"name":"amplihack-user-20250101","size":"Standard_D2s_v3","region":"eastus"},
            {"name":"other-vm","size":"Standard_D4s_v3","region":"westus"}
        ]"#;
        let vms = parse_azlin_list_json(json);
        assert_eq!(vms.len(), 2);
        assert_eq!(vms[0].name, "amplihack-user-20250101");
    }

    #[test]
    fn parse_azlin_text() {
        let text = "NAME                SIZE              REGION\n\
                     amplihack-user-123  Standard_D2s_v3   eastus\n";
        let vms = parse_azlin_list_text(text);
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].size, "Standard_D2s_v3");
    }

    #[test]
    fn parse_timestamp_formats() {
        assert!(parse_timestamp("2025-01-15 10:30:00").is_some());
        assert!(parse_timestamp("2025-01-15T10:30:00").is_some());
        assert!(parse_timestamp("20250115-103000").is_some());
        assert!(parse_timestamp("invalid").is_none());
    }
}
