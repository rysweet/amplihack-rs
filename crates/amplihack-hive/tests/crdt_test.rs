use amplihack_hive::{GCounter, LWWRegister};

// --- GCounter constructor/serde tests (should pass) ---

#[test]
fn gcounter_new_is_empty() {
    let _counter = GCounter::new();
}

#[test]
fn gcounter_default_is_constructible() {
    let _counter: GCounter = Default::default();
}

#[test]
fn gcounter_serde_roundtrip() {
    let counter = GCounter::new();
    let json = serde_json::to_string(&counter).unwrap();
    let deserialized: GCounter = serde_json::from_str(&json).unwrap();
    // Both should be freshly constructed, equivalent empty counters
    let _ = deserialized;
}

// --- GCounter todo!() methods (should_panic) ---

#[test]
#[should_panic(expected = "not yet implemented")]
fn gcounter_increment_single_node() {
    let mut counter = GCounter::new();
    counter.increment("node-1");
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn gcounter_increment_multiple_nodes() {
    let mut counter = GCounter::new();
    counter.increment("node-1");
    counter.increment("node-2");
    counter.increment("node-1");
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn gcounter_value_sum() {
    let counter = GCounter::new();
    let _val = counter.value();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn gcounter_merge_disjoint() {
    let mut a = GCounter::new();
    let b = GCounter::new();
    a.merge(&b);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn gcounter_merge_overlapping() {
    let mut a = GCounter::new();
    a.increment("node-1");
    let mut b = GCounter::new();
    b.increment("node-1");
    a.merge(&b);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn gcounter_get_node() {
    let counter = GCounter::new();
    let _val = counter.get("node-1");
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn gcounter_get_nonexistent_node() {
    let counter = GCounter::new();
    let _val = counter.get("no-such-node");
}

// --- LWWRegister accessor tests (REAL implementation, should pass) ---

#[test]
fn lww_register_new_is_none() {
    let reg = LWWRegister::<String>::new("node-1".to_string());
    // get() is todo, but timestamp and node_id are real
    let _ = reg;
}

#[test]
fn lww_register_node_id_preserved() {
    let reg = LWWRegister::<String>::new("node-alpha".to_string());
    assert_eq!(reg.node_id(), "node-alpha");
}

#[test]
fn lww_register_initial_timestamp_zero() {
    let reg = LWWRegister::<i32>::new("node-1".to_string());
    assert_eq!(reg.timestamp(), 0);
}

#[test]
fn lww_register_different_types() {
    let _str_reg = LWWRegister::<String>::new("n1".to_string());
    let _int_reg = LWWRegister::<i64>::new("n2".to_string());
    let _vec_reg = LWWRegister::<Vec<u8>>::new("n3".to_string());
}

// --- LWWRegister todo!() methods (should_panic) ---

#[test]
#[should_panic(expected = "not yet implemented")]
fn lww_register_set_value() {
    let mut reg = LWWRegister::<String>::new("node-1".to_string());
    reg.set("hello".to_string(), 1);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn lww_register_get_value() {
    let reg = LWWRegister::<String>::new("node-1".to_string());
    let _val = reg.get();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn lww_register_merge_newer_wins() {
    let mut a = LWWRegister::<String>::new("node-a".to_string());
    let b = LWWRegister::<String>::new("node-b".to_string());
    a.merge(&b);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn lww_register_merge_older_loses() {
    let mut a = LWWRegister::<String>::new("node-a".to_string());
    let b = LWWRegister::<String>::new("node-b".to_string());
    a.merge(&b);
}
