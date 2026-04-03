use amplihack_hive::{GCounter, LWWRegister};

// --- GCounter constructor/serde tests (should pass) ---

#[test]
fn gcounter_new_is_empty() {
    let counter = GCounter::new();
    assert_eq!(counter.value(), 0);
}

#[test]
fn gcounter_default_is_constructible() {
    let counter: GCounter = Default::default();
    assert_eq!(counter.value(), 0);
}

#[test]
fn gcounter_serde_roundtrip() {
    let mut counter = GCounter::new();
    counter.increment("node-1");
    counter.increment("node-2");
    let json = serde_json::to_string(&counter).unwrap();
    let deserialized: GCounter = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.value(), counter.value());
    assert_eq!(deserialized.get("node-1"), 1);
    assert_eq!(deserialized.get("node-2"), 1);
}

// --- GCounter behavioral tests ---

#[test]
fn gcounter_increment_single_node() {
    let mut counter = GCounter::new();
    let val = counter.increment("node-1");
    assert_eq!(val, 1);
    assert_eq!(counter.value(), 1);
    let val2 = counter.increment("node-1");
    assert_eq!(val2, 2);
    assert_eq!(counter.value(), 2);
}

#[test]
fn gcounter_increment_multiple_nodes() {
    let mut counter = GCounter::new();
    counter.increment("node-1");
    counter.increment("node-2");
    counter.increment("node-1");
    assert_eq!(counter.get("node-1"), 2);
    assert_eq!(counter.get("node-2"), 1);
    assert_eq!(counter.value(), 3);
}

#[test]
fn gcounter_value_sum() {
    let counter = GCounter::new();
    assert_eq!(counter.value(), 0);
}

#[test]
fn gcounter_merge_disjoint() {
    let mut a = GCounter::new();
    a.increment("node-a");
    let mut b = GCounter::new();
    b.increment("node-b");
    a.merge(&b);
    assert_eq!(a.get("node-a"), 1);
    assert_eq!(a.get("node-b"), 1);
    assert_eq!(a.value(), 2);
}

#[test]
fn gcounter_merge_overlapping() {
    let mut a = GCounter::new();
    a.increment("node-1");
    let mut b = GCounter::new();
    b.increment("node-1");
    b.increment("node-1");
    b.increment("node-1");
    a.merge(&b);
    // max(1, 3) = 3
    assert_eq!(a.get("node-1"), 3);
    assert_eq!(a.value(), 3);
}

#[test]
fn gcounter_get_node() {
    let mut counter = GCounter::new();
    counter.increment("node-1");
    counter.increment("node-1");
    assert_eq!(counter.get("node-1"), 2);
}

#[test]
fn gcounter_get_nonexistent_node() {
    let counter = GCounter::new();
    assert_eq!(counter.get("no-such-node"), 0);
}

// --- LWWRegister accessor tests ---

#[test]
fn lww_register_new_is_none() {
    let reg = LWWRegister::<String>::new("node-1".to_string());
    assert!(reg.get().is_none());
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
    let str_reg = LWWRegister::<String>::new("n1".to_string());
    let int_reg = LWWRegister::<i64>::new("n2".to_string());
    let vec_reg = LWWRegister::<Vec<u8>>::new("n3".to_string());
    assert!(str_reg.get().is_none());
    assert!(int_reg.get().is_none());
    assert!(vec_reg.get().is_none());
}

// --- LWWRegister behavioral tests ---

#[test]
fn lww_register_set_value() {
    let mut reg = LWWRegister::<String>::new("node-1".to_string());
    reg.set("hello".to_string(), 1);
    assert_eq!(reg.get(), Some(&"hello".to_string()));
    assert_eq!(reg.timestamp(), 1);
}

#[test]
fn lww_register_get_value() {
    let reg = LWWRegister::<String>::new("node-1".to_string());
    assert!(reg.get().is_none());
}

#[test]
fn lww_register_merge_newer_wins() {
    let mut a = LWWRegister::<String>::new("node-a".to_string());
    a.set("old".to_string(), 1);
    let mut b = LWWRegister::<String>::new("node-b".to_string());
    b.set("new".to_string(), 5);
    a.merge(&b);
    assert_eq!(a.get(), Some(&"new".to_string()));
    assert_eq!(a.timestamp(), 5);
}

#[test]
fn lww_register_merge_older_loses() {
    let mut a = LWWRegister::<String>::new("node-a".to_string());
    a.set("newer".to_string(), 10);
    let mut b = LWWRegister::<String>::new("node-b".to_string());
    b.set("older".to_string(), 3);
    a.merge(&b);
    assert_eq!(a.get(), Some(&"newer".to_string()));
    assert_eq!(a.timestamp(), 10);
}
