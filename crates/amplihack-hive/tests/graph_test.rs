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

// --- new: hive_id tests ---

#[test]
fn graph_with_id() {
    let g = HiveGraph::with_id("my-hive");
    assert_eq!(g.hive_id(), "my-hive");
}

// --- new: retract_fact tests ---

#[test]
fn retract_fact_sets_status() {
    let mut g = HiveGraph::new();
    let id = g
        .store_fact("rust", "Rust is fast", 0.9, "a", vec![])
        .unwrap();
    assert!(g.retract_fact(&id, "outdated"));
    let fact = g.get_fact(&id).unwrap().unwrap();
    assert_eq!(fact.status, "retracted");
    assert_eq!(fact.metadata.get("retraction_reason").unwrap(), "outdated");
}

#[test]
fn retract_nonexistent_returns_false() {
    let mut g = HiveGraph::new();
    assert!(!g.retract_fact("no-such-id", "reason"));
}

#[test]
fn query_excludes_retracted_facts() {
    let mut g = HiveGraph::new();
    let id = g
        .store_fact("rust", "Rust is fast", 0.9, "a", vec![])
        .unwrap();
    g.retract_fact(&id, "wrong");
    assert!(g.query_facts("rust", 0.0, 10).unwrap().is_empty());
}

// --- new: agent registry tests ---

#[test]
fn register_and_get_agent() {
    let mut g = HiveGraph::new();
    g.register_agent("agent-1", "biology").unwrap();
    let agent = g.get_agent("agent-1").unwrap();
    assert_eq!(agent.domain, "biology");
    assert_eq!(agent.status, "active");
}

#[test]
fn register_duplicate_agent_fails() {
    let mut g = HiveGraph::new();
    g.register_agent("a1", "bio").unwrap();
    assert!(g.register_agent("a1", "phys").is_err());
}

#[test]
fn unregister_agent_sets_removed() {
    let mut g = HiveGraph::new();
    g.register_agent("a1", "bio").unwrap();
    assert!(g.unregister_agent("a1"));
    assert_eq!(g.get_agent("a1").unwrap().status, "removed");
}

#[test]
fn list_agents_filtered() {
    let mut g = HiveGraph::new();
    g.register_agent("a", "bio").unwrap();
    g.register_agent("b", "phys").unwrap();
    g.unregister_agent("b");
    assert_eq!(g.list_agents(Some("active")).len(), 1);
    assert_eq!(g.list_agents(None).len(), 2);
}

#[test]
fn update_trust_clamps() {
    let mut g = HiveGraph::new();
    g.register_agent("a", "bio").unwrap();
    g.update_trust("a", 3.0);
    assert!((g.get_agent("a").unwrap().trust - 2.0).abs() < f64::EPSILON);
}

// --- new: edge tests ---

#[test]
fn add_and_get_edges() {
    let mut g = HiveGraph::new();
    g.add_edge("f1", "f2", "CONTRADICTS", std::collections::HashMap::new());
    assert_eq!(g.get_edges("f1").len(), 1);
    assert_eq!(g.get_edges("f2").len(), 1);
}

#[test]
fn get_edges_from_specific_type() {
    let mut g = HiveGraph::new();
    g.add_edge("f1", "f2", "CONTRADICTS", std::collections::HashMap::new());
    g.add_edge("f1", "f3", "CONFIRMED_BY", std::collections::HashMap::new());
    assert_eq!(g.get_edges_from("f1", "CONTRADICTS").len(), 1);
}

// --- new: keyword search tests ---

use amplihack_hive::{tokenize, word_overlap};

#[test]
fn tokenize_filters_short() {
    let tokens = tokenize("Hello World of Rust");
    assert!(tokens.contains("hello"));
    assert!(!tokens.contains("a")); // single char filtered
}

#[test]
fn word_overlap_identical() {
    assert!((word_overlap("rust is fast", "rust is fast") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn word_overlap_disjoint() {
    assert!(word_overlap("rust is fast", "python is slow") < 0.5);
}

#[test]
fn keyword_query_finds_relevant() {
    let mut g = HiveGraph::new();
    g.store_fact("lang", "Rust is a systems language", 0.9, "a", vec![])
        .unwrap();
    g.store_fact("lang", "Python is interpreted", 0.8, "a", vec![])
        .unwrap();
    let results = g.keyword_query("systems language", 10);
    assert!(!results.is_empty());
    assert!(results[0].fact.content.contains("systems"));
}

// --- new: contradiction detection tests ---

#[test]
fn check_contradictions_finds_overlapping() {
    let mut g = HiveGraph::new();
    g.store_fact(
        "water",
        "water boils at 100 degrees Celsius",
        0.9,
        "a",
        vec![],
    )
    .unwrap();
    let contras = g.check_contradictions("water", "water boils at 50 degrees Celsius");
    assert_eq!(contras.len(), 1);
}

#[test]
fn check_contradictions_ignores_different_concept() {
    let mut g = HiveGraph::new();
    g.store_fact("water", "water boils at 100C", 0.9, "a", vec![])
        .unwrap();
    assert!(g.check_contradictions("ice", "ice melts at 0C").is_empty());
}

// --- new: query routing tests ---

#[test]
fn route_query_to_domain_experts() {
    let mut g = HiveGraph::new();
    g.register_agent("bio-agent", "biology genetics").unwrap();
    g.register_agent("phys-agent", "physics quantum").unwrap();
    let routed = g.route_query("genetics research");
    assert_eq!(routed.len(), 1);
    assert_eq!(routed[0], "bio-agent");
}

// --- new: federation tests ---

#[test]
fn federation_parent_child() {
    let mut g = HiveGraph::with_id("child");
    g.set_parent("parent-hive");
    assert_eq!(g.parent_id(), Some("parent-hive"));
    g.add_child("sub-1");
    g.add_child("sub-2");
    assert_eq!(g.children_ids().len(), 2);
}

#[test]
fn escalate_fact_to_parent() {
    let mut child = HiveGraph::with_id("child");
    let mut parent = HiveGraph::with_id("parent");
    let id = child
        .store_fact("bio", "DNA stores info", 0.9, "a", vec![])
        .unwrap();
    let new_id = child.escalate_fact(&id, &mut parent);
    assert!(new_id.is_some());
    assert_eq!(parent.fact_count(), 1);
}

#[test]
fn escalate_prevents_loop() {
    let mut child = HiveGraph::with_id("child");
    let mut parent = HiveGraph::with_id("parent");
    let id = child
        .store_fact("bio", "x", 0.9, "a", vec!["escalation:other".into()])
        .unwrap();
    assert!(child.escalate_fact(&id, &mut parent).is_none());
}

#[test]
fn broadcast_fact_to_children() {
    let mut parent = HiveGraph::with_id("parent");
    let c1 = HiveGraph::with_id("c1");
    let c2 = HiveGraph::with_id("c2");
    let id = parent
        .store_fact("bio", "DNA info", 0.9, "a", vec![])
        .unwrap();
    let result = parent.broadcast_fact(&id, &mut [c1, c2]);
    assert_eq!(result.len(), 2);
}

// --- new: get_stats tests ---

#[test]
fn get_stats_reports_correctly() {
    let mut g = HiveGraph::new();
    g.register_agent("a", "bio").unwrap();
    g.store_fact("bio", "fact 1", 0.9, "a", vec![]).unwrap();
    let id2 = g.store_fact("bio", "fact 2", 0.8, "a", vec![]).unwrap();
    g.retract_fact(&id2, "wrong");
    g.add_edge("f1", "f2", "X", std::collections::HashMap::new());
    let stats = g.get_stats();
    assert_eq!(stats.fact_count, 2);
    assert_eq!(stats.retracted_count, 1);
    assert_eq!(stats.agent_count, 1);
    assert_eq!(stats.edge_count, 1);
}
