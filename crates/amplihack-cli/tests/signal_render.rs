//! TDD contract — `amplihack signal setup` link-URI rendering (#921/#922).
//!
//! Run with: `cargo test -p amplihack-cli --features signal --test signal_render`
//!
//! These tests define the contract for `commands::signal::render`, a PURE,
//! I/O-free module that turns a Signal device-link URI into terminal output.
//! Requirements exercised here:
//!   * A scannable QR code is rendered (Unicode block glyphs present).
//!   * The RAW URI is ALWAYS printed verbatim as a copy/paste fallback.
//!   * Rendering is deterministic (same input → identical output).
//!   * The renderer is scheme-agnostic: it encodes whatever signal-cli emits
//!     — modern `sgnl://linkdevice?...` AND legacy `tsdevice:/?...`.
//!
//! Rendering must be a pure function so it is fully unit-testable with no
//! terminal, clock, or process dependency.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::render;

const MODERN_URI: &str =
    "sgnl://linkdevice?uuid=abcd-1234&pub_key=BASE64PUBKEYVALUEHERE0000000000000000000000";
const LEGACY_URI: &str =
    "tsdevice:/?uuid=abcd-1234&pub_key=BASE64PUBKEYVALUEHERE0000000000000000000000";

/// Any recognizable QR block glyph proves a code was drawn (half-block or full
/// block rendering styles both use these).
fn contains_qr_glyph(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '█' | '▀' | '▄' | '▐' | '▌' | '■' | '□' | ' '))
        && s.contains('█') // a real module fill must appear somewhere
}

#[test]
fn render_link_embeds_raw_uri_verbatim_as_fallback() {
    let out = render::render_link(MODERN_URI);
    assert!(
        out.contains(MODERN_URI),
        "rendered output MUST include the raw URI verbatim as a fallback; got:\n{out}"
    );
}

#[test]
fn render_link_draws_a_qr_code() {
    let out = render::render_link(MODERN_URI);
    assert!(
        contains_qr_glyph(&out),
        "rendered output MUST contain QR block glyphs (a scannable code); got:\n{out}"
    );
}

#[test]
fn render_link_is_deterministic() {
    let a = render::render_link(MODERN_URI);
    let b = render::render_link(MODERN_URI);
    assert_eq!(a, b, "render_link must be a pure, deterministic function");
}

#[test]
fn render_link_is_non_empty() {
    assert!(!render::render_link(MODERN_URI).trim().is_empty());
}

#[test]
fn render_link_supports_legacy_tsdevice_scheme() {
    let out = render::render_link(LEGACY_URI);
    assert!(
        out.contains(LEGACY_URI),
        "legacy tsdevice URI must still be rendered verbatim; got:\n{out}"
    );
    assert!(
        contains_qr_glyph(&out),
        "legacy tsdevice URI must still produce a QR code; got:\n{out}"
    );
}

#[test]
fn different_uris_render_differently() {
    // Guards against a stub that ignores its input.
    assert_ne!(
        render::render_link(MODERN_URI),
        render::render_link(LEGACY_URI)
    );
}
