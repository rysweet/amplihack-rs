//! TLC trace-to-test generator (ported from Python PR #3959).
//!
//! Parses TLC DOT state graphs and generates pytest test cases for
//! hive-mind distributed retrieval verification.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

// ── Core types ───────────────────────────────────────────────────────────────

/// A single state in a TLC state graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TLCState {
    pub name: String,
    pub variables: BTreeMap<String, String>,
}

impl TLCState {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            variables: BTreeMap::new(),
        }
    }

    pub fn with_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }
}

/// A transition between two states in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TLCTransition {
    pub from_state: String,
    pub to_state: String,
}

/// A parsed TLC state graph with states, transitions, and boundary sets.
#[derive(Debug, Clone)]
pub struct TLCGraph {
    pub states: HashMap<String, TLCState>,
    pub transitions: Vec<TLCTransition>,
    pub initial_states: HashSet<String>,
    pub terminal_states: HashSet<String>,
}

impl TLCGraph {
    fn new() -> Self {
        Self {
            states: HashMap::new(),
            transitions: Vec::new(),
            initial_states: HashSet::new(),
            terminal_states: HashSet::new(),
        }
    }
}

// ── DOT parser ───────────────────────────────────────────────────────────────

/// Parse a DOT-format state graph into a [`TLCGraph`].
///
/// Expects a simplified DOT dialect:
/// - Node declarations: `"state_name" [label="var1=val1\nvar2=val2"]`
/// - Edge declarations: `"from" -> "to"`
/// - Lines starting with `//` or containing `digraph`/`}` are ignored.
pub fn parse_dot(input: &str) -> TLCGraph {
    let mut graph = TLCGraph::new();
    let mut from_counts: HashMap<String, usize> = HashMap::new();
    let mut to_counts: HashMap<String, usize> = HashMap::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("digraph")
            || trimmed == "}"
            || trimmed == "{"
        {
            continue;
        }

        if let Some(transition) = parse_edge(trimmed) {
            *from_counts
                .entry(transition.from_state.clone())
                .or_default() += 1;
            *to_counts.entry(transition.to_state.clone()).or_default() += 1;
            graph.transitions.push(transition);
        } else if let Some(state) = parse_node(trimmed) {
            graph.states.insert(state.name.clone(), state);
        }
    }

    // Initial states: appear as source but never as target.
    for name in from_counts.keys() {
        if !to_counts.contains_key(name) {
            graph.initial_states.insert(name.clone());
        }
    }

    // Terminal states: appear as target but never as source.
    for name in to_counts.keys() {
        if !from_counts.contains_key(name) {
            graph.terminal_states.insert(name.clone());
        }
    }

    // Nodes with no edges at all are both initial and terminal.
    for name in graph.states.keys() {
        if !from_counts.contains_key(name) && !to_counts.contains_key(name) {
            graph.initial_states.insert(name.clone());
            graph.terminal_states.insert(name.clone());
        }
    }

    graph
}

/// Parse a DOT node line like `"S1" [label="x=1\ny=2"]`.
fn parse_node(line: &str) -> Option<TLCState> {
    let line = line.trim().trim_end_matches(';');
    // Extract the node name between quotes
    let name_start = line.find('"')? + 1;
    let name_end = line[name_start..].find('"')? + name_start;
    let name = &line[name_start..name_end];

    let mut state = TLCState::new(name);

    // Parse label if present
    if let Some(label_start) = line.find("label=\"") {
        let label_start = label_start + 7;
        if let Some(label_end) = line[label_start..].find('"') {
            let label = &line[label_start..label_start + label_end];
            for part in label.split("\\n") {
                let part = part.trim();
                if let Some(eq_pos) = part.find('=') {
                    let key = part[..eq_pos].trim();
                    let value = part[eq_pos + 1..].trim();
                    if !key.is_empty() {
                        state.variables.insert(key.to_string(), value.to_string());
                    }
                }
            }
        }
    }

    Some(state)
}

/// Parse a DOT edge line like `"S1" -> "S2"`.
fn parse_edge(line: &str) -> Option<TLCTransition> {
    let line = line.trim().trim_end_matches(';');
    let arrow_pos = line.find("->")?;
    let from_part = line[..arrow_pos].trim();
    let to_part = line[arrow_pos + 2..].trim();

    let from = extract_quoted(from_part)?;
    // to_part may have trailing attributes like [label="..."]
    let to_clean = to_part.split('[').next().unwrap_or(to_part).trim();
    let to = extract_quoted(to_clean)?;

    Some(TLCTransition {
        from_state: from.to_string(),
        to_state: to.to_string(),
    })
}

fn extract_quoted(s: &str) -> Option<&str> {
    let start = s.find('"')? + 1;
    let end = s[start..].find('"')? + start;
    Some(&s[start..end])
}

// ── Trace extraction ─────────────────────────────────────────────────────────

/// Extract execution traces from a [`TLCGraph`] via BFS.
///
/// Returns up to `max` distinct paths from initial to terminal states.
/// Each path is a sequence of [`TLCState`] values.
pub fn extract_traces(graph: &TLCGraph, max: usize) -> Vec<Vec<TLCState>> {
    if graph.initial_states.is_empty() {
        return Vec::new();
    }

    // Build adjacency list
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in &graph.transitions {
        adjacency
            .entry(t.from_state.as_str())
            .or_default()
            .push(t.to_state.as_str());
    }

    let mut traces = Vec::new();
    let mut queue: VecDeque<Vec<String>> = VecDeque::new();

    for init in &graph.initial_states {
        queue.push_back(vec![init.clone()]);
    }

    while let Some(path) = queue.pop_front() {
        if traces.len() >= max {
            break;
        }

        let current = path.last().unwrap();

        if graph.terminal_states.contains(current.as_str()) {
            let trace: Vec<TLCState> = path
                .iter()
                .filter_map(|name| graph.states.get(name).cloned())
                .collect();
            if trace.len() == path.len() {
                traces.push(trace);
            }
            continue;
        }

        if let Some(neighbors) = adjacency.get(current.as_str()) {
            for next in neighbors {
                // Avoid cycles: don't revisit nodes in the current path
                if !path.contains(&next.to_string()) {
                    let mut new_path = path.clone();
                    new_path.push(next.to_string());
                    queue.push_back(new_path);
                }
            }
        }
    }

    traces
}

// ── Test code generation ─────────────────────────────────────────────────────

/// Generate pytest test code from extracted traces.
///
/// Each trace becomes a separate test function verifying the state transitions
/// of a hive-mind distributed retrieval run.
pub fn generate_test_code(traces: &[Vec<TLCState>]) -> String {
    let mut out = String::new();
    out.push_str(
        "\"\"\"Auto-generated TLC trace tests for hive-mind distributed retrieval.\"\"\"\n",
    );
    out.push_str("import pytest\n\n\n");

    if traces.is_empty() {
        out.push_str("# No traces extracted from the state graph.\n");
        return out;
    }

    for (i, trace) in traces.iter().enumerate() {
        out.push_str(&format!("def test_trace_{i}():\n"));
        out.push_str(&format!(
            "    \"\"\"Verify trace {i} ({} states).\"\"\"\n",
            trace.len()
        ));
        out.push_str("    states = [\n");
        for state in trace {
            let vars: Vec<String> = state
                .variables
                .iter()
                .map(|(k, v)| format!("\"{k}\": \"{v}\""))
                .collect();
            out.push_str(&format!(
                "        {{\"name\": \"{}\", \"vars\": {{{}}}}},\n",
                state.name,
                vars.join(", ")
            ));
        }
        out.push_str("    ]\n");
        out.push_str("    assert len(states) > 0\n");
        out.push_str("    for i in range(len(states) - 1):\n");
        out.push_str("        assert states[i][\"name\"] != states[i + 1][\"name\"]\n");
        out.push_str("\n\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DOT: &str = r#"
digraph state_graph {
    "Init" [label="phase=start\ncount=0"]
    "Retrieve" [label="phase=retrieve\ncount=1"]
    "Merge" [label="phase=merge\ncount=2"]
    "Done" [label="phase=done\ncount=3"]
    "Init" -> "Retrieve"
    "Retrieve" -> "Merge"
    "Merge" -> "Done"
}
"#;

    #[test]
    fn parse_dot_extracts_states() {
        let graph = parse_dot(SAMPLE_DOT);
        assert_eq!(graph.states.len(), 4);
        assert!(graph.states.contains_key("Init"));
        assert!(graph.states.contains_key("Done"));
    }

    #[test]
    fn parse_dot_extracts_transitions() {
        let graph = parse_dot(SAMPLE_DOT);
        assert_eq!(graph.transitions.len(), 3);
        assert_eq!(graph.transitions[0].from_state, "Init");
        assert_eq!(graph.transitions[0].to_state, "Retrieve");
    }

    #[test]
    fn parse_dot_identifies_initial_states() {
        let graph = parse_dot(SAMPLE_DOT);
        assert_eq!(graph.initial_states.len(), 1);
        assert!(graph.initial_states.contains("Init"));
    }

    #[test]
    fn parse_dot_identifies_terminal_states() {
        let graph = parse_dot(SAMPLE_DOT);
        assert_eq!(graph.terminal_states.len(), 1);
        assert!(graph.terminal_states.contains("Done"));
    }

    #[test]
    fn parse_dot_state_variables() {
        let graph = parse_dot(SAMPLE_DOT);
        let init = &graph.states["Init"];
        assert_eq!(init.variables["phase"], "start");
        assert_eq!(init.variables["count"], "0");
    }

    #[test]
    fn parse_node_with_label() {
        let state = parse_node(r#""S1" [label="x=1\ny=hello"]"#).unwrap();
        assert_eq!(state.name, "S1");
        assert_eq!(state.variables["x"], "1");
        assert_eq!(state.variables["y"], "hello");
    }

    #[test]
    fn parse_node_no_label() {
        let state = parse_node(r#""Bare""#).unwrap();
        assert_eq!(state.name, "Bare");
        assert!(state.variables.is_empty());
    }

    #[test]
    fn parse_edge_simple() {
        let edge = parse_edge(r#""A" -> "B""#).unwrap();
        assert_eq!(edge.from_state, "A");
        assert_eq!(edge.to_state, "B");
    }

    #[test]
    fn parse_edge_with_attrs() {
        let edge = parse_edge(r#""A" -> "B" [label="step"];"#).unwrap();
        assert_eq!(edge.from_state, "A");
        assert_eq!(edge.to_state, "B");
    }

    #[test]
    fn extract_traces_linear() {
        let graph = parse_dot(SAMPLE_DOT);
        let traces = extract_traces(&graph, 10);
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].len(), 4);
        assert_eq!(traces[0][0].name, "Init");
        assert_eq!(traces[0][3].name, "Done");
    }

    #[test]
    fn extract_traces_respects_max() {
        let dot = r#"
digraph g {
    "A" [label="v=1"]
    "B" [label="v=2"]
    "C" [label="v=3"]
    "D" [label="v=4"]
    "A" -> "B"
    "A" -> "C"
    "B" -> "D"
    "C" -> "D"
}
"#;
        let graph = parse_dot(dot);
        let traces = extract_traces(&graph, 1);
        assert_eq!(traces.len(), 1);
    }

    #[test]
    fn extract_traces_handles_branch() {
        let dot = r#"
digraph g {
    "A" [label="v=1"]
    "B" [label="v=2"]
    "C" [label="v=3"]
    "D" [label="v=4"]
    "A" -> "B"
    "A" -> "C"
    "B" -> "D"
    "C" -> "D"
}
"#;
        let graph = parse_dot(dot);
        let traces = extract_traces(&graph, 10);
        assert_eq!(traces.len(), 2);
    }

    #[test]
    fn extract_traces_avoids_cycles() {
        let dot = r#"
digraph g {
    "A" [label="v=1"]
    "B" [label="v=2"]
    "A" -> "B"
    "B" -> "A"
}
"#;
        let graph = parse_dot(dot);
        // With a cycle and no terminal states, no complete traces should be found.
        let traces = extract_traces(&graph, 10);
        assert!(traces.is_empty());
    }

    #[test]
    fn extract_traces_empty_graph() {
        let graph = TLCGraph::new();
        let traces = extract_traces(&graph, 10);
        assert!(traces.is_empty());
    }

    #[test]
    fn generate_test_code_produces_valid_python() {
        let graph = parse_dot(SAMPLE_DOT);
        let traces = extract_traces(&graph, 10);
        let code = generate_test_code(&traces);
        assert!(code.contains("def test_trace_0():"));
        assert!(code.contains("import pytest"));
        assert!(code.contains("assert len(states) > 0"));
        assert!(code.contains("\"name\": \"Init\""));
        assert!(code.contains("\"name\": \"Done\""));
    }

    #[test]
    fn generate_test_code_empty_traces() {
        let code = generate_test_code(&[]);
        assert!(code.contains("No traces extracted"));
    }

    #[test]
    fn tlc_state_builder() {
        let state = TLCState::new("test").with_var("a", "1").with_var("b", "2");
        assert_eq!(state.name, "test");
        assert_eq!(state.variables.len(), 2);
    }

    #[test]
    fn isolated_node_is_both_initial_and_terminal() {
        let dot = r#"
digraph g {
    "Alone" [label="status=idle"]
}
"#;
        let graph = parse_dot(dot);
        assert!(graph.initial_states.contains("Alone"));
        assert!(graph.terminal_states.contains("Alone"));
    }

    #[test]
    fn parse_dot_semicolons() {
        let dot = r#"
digraph g {
    "X" [label="k=v"];
    "Y" [label="k=w"];
    "X" -> "Y";
}
"#;
        let graph = parse_dot(dot);
        assert_eq!(graph.states.len(), 2);
        assert_eq!(graph.transitions.len(), 1);
    }
}
