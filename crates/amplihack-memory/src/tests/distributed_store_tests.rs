use super::*;
use serde_json::json;

fn make_props(content: &str) -> Props {
    let mut p = Props::new();
    p.insert("content".into(), json!(content));
    p
}

#[test]
fn add_agents_and_create_node() {
    let mut store = DistributedGraphStore::new(DistributedConfig::default());
    store.add_agent("a1");
    store.add_agent("a2");
    let id = store
        .create_node("test", &make_props("hello world"))
        .unwrap();
    let node = store.get_node("test", &id).unwrap();
    assert!(node.is_some());
}

#[test]
fn search_across_shards() {
    let mut store = DistributedGraphStore::new(DistributedConfig {
        replication_factor: 1,
        query_fanout: 10,
    });
    store.add_agent("a1");
    store.add_agent("a2");
    store.create_node("t", &make_props("sky is blue")).unwrap();
    store
        .create_node("t", &make_props("grass is green"))
        .unwrap();
    let results = store.search_nodes("t", "sky", None, 10).unwrap();
    assert!(!results.is_empty());
}

#[test]
fn gossip_syncs_between_shards() {
    let mut store = DistributedGraphStore::new(DistributedConfig {
        replication_factor: 1,
        query_fanout: 10,
    });
    store.add_agent("a1");
    store.add_agent("a2");

    // Create nodes only on a1's shard
    store
        .shards
        .get_mut("a1")
        .unwrap()
        .store
        .create_node("t", &make_props("exclusive data"))
        .unwrap();

    let (nodes, _) = store.run_gossip_round().unwrap();
    assert!(nodes > 0, "gossip should sync at least one node");
}

#[test]
fn rebuild_shard_recovers_data() {
    let mut store = DistributedGraphStore::new(DistributedConfig {
        replication_factor: 1,
        query_fanout: 10,
    });
    store.add_agent("a1");
    store.add_agent("a2");

    // Add data to a1
    store
        .shards
        .get_mut("a1")
        .unwrap()
        .store
        .create_node("t", &make_props("data to recover"))
        .unwrap();

    let recovered = store.rebuild_shard("a2").unwrap();
    assert!(recovered > 0);
}

#[test]
fn shard_stats() {
    let mut store = DistributedGraphStore::new(DistributedConfig::default());
    store.add_agent("a1");
    store.create_node("t", &make_props("test data")).unwrap();
    let stats = store.shard_stats("a1").unwrap();
    assert!(stats.bloom_size_bytes > 0);
}

#[test]
fn remove_agent() {
    let mut store = DistributedGraphStore::new(DistributedConfig::default());
    store.add_agent("a1");
    store.add_agent("a2");
    assert_eq!(store.agent_count(), 2);
    store.remove_agent("a1");
    assert_eq!(store.agent_count(), 1);
}

#[test]
fn no_shards_returns_error() {
    let mut store = DistributedGraphStore::new(DistributedConfig::default());
    let result = store.create_node("t", &make_props("orphan"));
    assert!(result.is_err());
}

#[test]
fn create_node_generates_unique_ids() {
    let mut store = DistributedGraphStore::new(DistributedConfig {
        replication_factor: 1,
        query_fanout: 10,
    });
    store.add_agent("a1");
    let id1 = store.create_node("t", &make_props("first")).unwrap();
    let id2 = store.create_node("t", &make_props("second")).unwrap();
    let id3 = store.create_node("t", &make_props("third")).unwrap();
    assert_ne!(id1, id2, "each node must get a unique ID");
    assert_ne!(id2, id3, "each node must get a unique ID");
    assert_ne!(id1, id3, "each node must get a unique ID");
}
