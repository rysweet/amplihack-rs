use amplihack_hive::HiveGraph;

// --- store_fact tests ---

#[test]
fn store_fact_returns_unique_id() {
    let mut graph = HiveGraph::new();
    let id1 = graph
        .store_fact("rust", "Rust is a systems language", 0.9, "agent-1", vec![])
        .unwrap();
    let id2 = graph
        .store_fact("python", "Python is interpreted", 0.8, "agent-1", vec![])
        .unwrap();
    assert!(!id1.is_empty());
    assert!(!id2.is_empty());
    assert_ne!(id1, id2);
}

#[test]
fn store_fact_with_tags() {
    let mut graph = HiveGraph::new();
    let id = graph
        .store_fact(
            "rust",
            "Rust has zero-cost abstractions",
            0.85,
            "agent-2",
            vec!["language".to_string(), "performance".to_string()],
        )
        .unwrap();
    let fact = graph.get_fact(&id).unwrap().unwrap();
    assert_eq!(fact.tags, vec!["language", "performance"]);
}

#[test]
fn store_fact_high_confidence() {
    let mut graph = HiveGraph::new();
    let id = graph
        .store_fact("math", "2 + 2 = 4", 1.0, "agent-1", vec![])
        .unwrap();
    let fact = graph.get_fact(&id).unwrap().unwrap();
    assert!((fact.confidence - 1.0).abs() < f64::EPSILON);
}

#[test]
fn store_fact_zero_confidence() {
    let mut graph = HiveGraph::new();
    let id = graph
        .store_fact("rumor", "unverified claim", 0.0, "agent-1", vec![])
        .unwrap();
    let fact = graph.get_fact(&id).unwrap().unwrap();
    assert!((fact.confidence).abs() < f64::EPSILON);
}

#[test]
fn store_fact_empty_concept() {
    let mut graph = HiveGraph::new();
    let id = graph
        .store_fact("", "no concept", 0.5, "agent-1", vec![])
        .unwrap();
    let fact = graph.get_fact(&id).unwrap().unwrap();
    assert_eq!(fact.concept, "");
}

// --- query_facts tests ---

#[test]
fn query_facts_by_concept() {
    let mut graph = HiveGraph::new();
    graph
        .store_fact("rust", "Rust is fast", 0.9, "src", vec![])
        .unwrap();
    graph
        .store_fact("python", "Python is easy", 0.8, "src", vec![])
        .unwrap();
    let facts = graph.query_facts("rust", 0.0, 10).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
}

#[test]
fn query_facts_filters_by_min_confidence() {
    let mut graph = HiveGraph::new();
    graph
        .store_fact("rust", "High confidence", 0.9, "src", vec![])
        .unwrap();
    graph
        .store_fact("rust", "Low confidence", 0.3, "src", vec![])
        .unwrap();
    let facts = graph.query_facts("rust", 0.8, 10).unwrap();
    assert_eq!(facts.len(), 1);
    assert!((facts[0].confidence - 0.9).abs() < f64::EPSILON);
}

#[test]
fn query_facts_respects_limit() {
    let mut graph = HiveGraph::new();
    graph
        .store_fact("rust", "Fact 1", 0.9, "src", vec![])
        .unwrap();
    graph
        .store_fact("rust", "Fact 2", 0.8, "src", vec![])
        .unwrap();
    graph
        .store_fact("rust", "Fact 3", 0.7, "src", vec![])
        .unwrap();
    let facts = graph.query_facts("rust", 0.0, 1).unwrap();
    assert_eq!(facts.len(), 1);
}

#[test]
fn query_facts_empty_concept() {
    let mut graph = HiveGraph::new();
    graph
        .store_fact("rust", "A fact", 0.9, "src", vec![])
        .unwrap();
    // Empty concept matches all (every string contains "")
    let facts = graph.query_facts("", 0.0, 10).unwrap();
    assert_eq!(facts.len(), 1);
}

// --- get_fact tests ---

#[test]
fn get_fact_existing() {
    let mut graph = HiveGraph::new();
    let id = graph
        .store_fact("rust", "Rust is fast", 0.9, "src", vec![])
        .unwrap();
    let fact = graph.get_fact(&id).unwrap();
    assert!(fact.is_some());
    assert_eq!(fact.unwrap().concept, "rust");
}

#[test]
fn get_fact_nonexistent() {
    let graph = HiveGraph::new();
    let fact = graph.get_fact("nonexistent-id").unwrap();
    assert!(fact.is_none());
}

// --- remove_fact tests ---

#[test]
fn remove_fact_existing() {
    let mut graph = HiveGraph::new();
    let id = graph
        .store_fact("rust", "Rust is fast", 0.9, "src", vec![])
        .unwrap();
    let removed = graph.remove_fact(&id).unwrap();
    assert!(removed);
    assert!(graph.get_fact(&id).unwrap().is_none());
}

#[test]
fn remove_fact_nonexistent() {
    let mut graph = HiveGraph::new();
    let removed = graph.remove_fact("no-such-fact").unwrap();
    assert!(!removed);
}

// --- facts_by_tag tests ---

#[test]
fn facts_by_tag_returns_matching() {
    let mut graph = HiveGraph::new();
    graph
        .store_fact(
            "rust",
            "Rust is a language",
            0.9,
            "src",
            vec!["language".to_string()],
        )
        .unwrap();
    graph
        .store_fact("math", "2+2=4", 0.9, "src", vec!["math".to_string()])
        .unwrap();
    let facts = graph.facts_by_tag("language").unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
}

#[test]
fn facts_by_tag_empty_result() {
    let graph = HiveGraph::new();
    let facts = graph.facts_by_tag("nonexistent-tag").unwrap();
    assert!(facts.is_empty());
}

// --- fact_count tests ---

#[test]
fn fact_count_empty_graph() {
    let graph = HiveGraph::new();
    assert_eq!(graph.fact_count(), 0);
}

#[test]
fn fact_count_after_insertions() {
    let mut graph = HiveGraph::new();
    graph.store_fact("a", "fact a", 0.5, "src", vec![]).unwrap();
    graph.store_fact("b", "fact b", 0.6, "src", vec![]).unwrap();
    assert_eq!(graph.fact_count(), 2);
}

// --- constructor / Default tests ---

#[test]
fn new_graph_is_constructible() {
    let _graph = HiveGraph::new();
}

#[test]
fn graph_default_is_constructible() {
    let _graph: HiveGraph = Default::default();
}
