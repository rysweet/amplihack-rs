use serde_yaml::Value;
use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root
}

#[test]
fn smart_orchestrator_active_steps_do_not_reference_orch_helper_py_or_helper_path() {
    let path = workspace_root().join("amplifier-bundle/recipes/smart-orchestrator.yaml");
    let raw =
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let yaml: Value =
        serde_yaml::from_str(&raw).unwrap_or_else(|err| panic!("parse {}: {err}", path.display()));
    let steps = yaml
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("smart-orchestrator must have a steps sequence");

    let mut violations = Vec::new();
    for step in steps {
        let Some(mapping) = step.as_mapping() else {
            continue;
        };
        let id = mapping
            .get(Value::String("id".to_string()))
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        for field in ["command", "script", "run"] {
            let Some(value) = mapping
                .get(Value::String(field.to_string()))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if value.contains("orch_helper.py")
                || value.contains("resolve-bundle-asset helper-path")
            {
                violations.push(format!("{id}.{field}: {value}"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "active smart-orchestrator executable fields must not depend on orch_helper.py/helper-path:\n{}",
        violations.join("\n")
    );
}
