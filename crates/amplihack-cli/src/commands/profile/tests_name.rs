use super::name::ProfileName;

#[test]
fn accepts_valid_names() {
    for n in [
        "work",
        "Work1",
        "a",
        "a.b",
        "a_b-c",
        "0abc",
        &"a".repeat(64),
    ] {
        let p = ProfileName::parse(n).unwrap_or_else(|e| panic!("{n} should be valid: {e:?}"));
        assert_eq!(p.as_str(), n);
    }
}

#[test]
fn rejects_malicious_and_invalid_names() {
    let too_long = "a".repeat(65);
    for n in [
        "", ".x", "-x", "a/b", "a\\b", "..", "a..b", "a\0b", "/abs", "../x", "a b", "a.", "CON",
        "con", "nul.txt", "PRN", "aux", "COM1", "lpt9.log", "é", "a\nb", &too_long,
    ] {
        assert!(ProfileName::parse(n).is_err(), "{n:?} must be rejected");
    }
}

#[test]
fn eq_fold_is_case_insensitive() {
    let a = ProfileName::parse("Work").unwrap();
    let b = ProfileName::parse("work").unwrap();
    let c = ProfileName::parse("home").unwrap();
    assert!(a.eq_fold(&b));
    assert!(!a.eq_fold(&c));
    assert_ne!(a, b, "equality itself stays case-sensitive");
}
