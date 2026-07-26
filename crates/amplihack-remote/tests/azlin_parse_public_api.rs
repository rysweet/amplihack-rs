//! Public-API contract (#921/#971 R4): the azlin discovery parsers and the
//! idle watchdog are re-exported from `amplihack_remote` so the Signal fleet
//! path consumes a single shared implementation. These tests fail to *compile*
//! if the re-exports regress to `pub(crate)`, and assert the parse semantics
//! the fleet path relies on (tolerant of malformed input, never panicking).

use amplihack_remote::{parse_azlin_list_json, parse_azlin_list_text};

#[test]
fn json_parser_is_public_and_extracts_names() {
    let json = r#"[
        {"name":"amplihack-user-1","size":"Standard_D2s_v3","region":"eastus"},
        {"name":"amplihack-user-2","size":"Standard_D4s_v3","region":"westus"}
    ]"#;
    let vms = parse_azlin_list_json(json);
    assert_eq!(vms.len(), 2);
    assert_eq!(vms[0].name, "amplihack-user-1");
    assert_eq!(vms[1].name, "amplihack-user-2");
}

#[test]
fn text_parser_is_public_and_skips_header() {
    let text = "NAME              SIZE             REGION\n\
                vm-alpha          Standard_D2s_v3  eastus\n\
                vm-beta           Standard_D4s_v3  westus\n";
    let vms = parse_azlin_list_text(text);
    assert_eq!(vms.len(), 2);
    assert_eq!(vms[0].name, "vm-alpha");
    assert_eq!(vms[1].region, "westus");
}

#[test]
fn malformed_json_never_panics_and_yields_empty() {
    assert!(parse_azlin_list_json("not json at all").is_empty());
    assert!(parse_azlin_list_json("{}").is_empty());
    assert!(parse_azlin_list_json("").is_empty());
}

#[test]
fn idle_watchdog_is_reexported() {
    // Compile-time proof that the watchdog surface is exposed for idle-based
    // device-linking. `IdleConfig::with_idle` is part of the public API.
    let _cfg =
        amplihack_remote::idle_watchdog::IdleConfig::with_idle(std::time::Duration::from_secs(1));
}
