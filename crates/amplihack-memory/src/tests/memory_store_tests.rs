use super::*;
use serde_json::json;

fn make_props(content: &str) -> Props {
    let mut p = Props::new();
    p.insert("content".into(), json!(content));
    p
}

#[test]
fn create_and_get_node() {
    let mut store = InMemoryGraphStore::new();
    let id = store.create_node("test", &make_props("hello")).unwrap();
    let node = store.get_node("test", &id).unwrap().unwrap();
    assert_eq!(node["content"], "hello");
}

#[test]
fn update_node() {
    let mut store = InMemoryGraphStore::new();
    let id = store.create_node("t", &make_props("v1")).unwrap();
    store.update_node("t", &id, &make_props("v2")).unwrap();
    let node = store.get_node("t", &id).unwrap().unwrap();
    assert_eq!(node["content"], "v2");
}

#[test]
fn delete_node_removes_edges() {
    let mut store = InMemoryGraphStore::new();
    let a = store.create_node("t", &make_props("a")).unwrap();
    let b = store.create_node("t", &make_props("b")).unwrap();
    store
        .create_edge("rel", "t", &a, "t", &b, &Props::new())
        .unwrap();
    store.delete_node("t", &a).unwrap();
    assert!(store.get_node("t", &a).unwrap().is_none());
    let edges = store.get_edges(&a, None, EdgeDirection::Both).unwrap();
    assert!(edges.is_empty());
}

#[test]
fn search_nodes_by_text() {
    let mut store = InMemoryGraphStore::new();
    store
        .create_node("t", &make_props("the sky is blue"))
        .unwrap();
    store
        .create_node("t", &make_props("grass is green"))
        .unwrap();
    let results = store.search_nodes("t", "sky", None, 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["content"], "the sky is blue");
}

#[test]
fn query_with_filters() {
    let mut store = InMemoryGraphStore::new();
    let mut p = make_props("x");
    p.insert("status".into(), json!("active"));
    store.create_node("t", &p).unwrap();
    store.create_node("t", &make_props("y")).unwrap();
    let filter: Props = [("status".into(), json!("active"))].into_iter().collect();
    let results = store.query_nodes("t", Some(&filter), 10).unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn export_import_round_trip() {
    let mut store = InMemoryGraphStore::new();
    let id = store.create_node("t", &make_props("data")).unwrap();
    let nodes = store.export_nodes(None).unwrap();
    let mut store2 = InMemoryGraphStore::new();
    let imported = store2.import_nodes(&nodes).unwrap();
    assert_eq!(imported, 1);
    assert!(store2.get_node("t", &id).unwrap().is_some());
    // Second import is idempotent
    let imported2 = store2.import_nodes(&nodes).unwrap();
    assert_eq!(imported2, 0);
}

#[test]
fn edge_directions() {
    let mut store = InMemoryGraphStore::new();
    let a = store.create_node("t", &make_props("a")).unwrap();
    let b = store.create_node("t", &make_props("b")).unwrap();
    store
        .create_edge("knows", "t", &a, "t", &b, &Props::new())
        .unwrap();
    assert_eq!(
        store
            .get_edges(&a, None, EdgeDirection::Outgoing)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .get_edges(&a, None, EdgeDirection::Incoming)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        store
            .get_edges(&b, None, EdgeDirection::Incoming)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .get_edges(&a, None, EdgeDirection::Both)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn get_all_node_ids() {
    let mut store = InMemoryGraphStore::new();
    store.create_node("t1", &make_props("a")).unwrap();
    store.create_node("t2", &make_props("b")).unwrap();
    let all = store.get_all_node_ids(None).unwrap();
    assert_eq!(all.len(), 2);
    let t1_only = store.get_all_node_ids(Some("t1")).unwrap();
    assert_eq!(t1_only.len(), 1);
}
