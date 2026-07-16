//! TDD tests: Verify that the Code Atlas workflow only renders DOT diagrams
//! for layers that actually ship a `<layer>.dot` file.
//!
//! ## Why this test exists (issue #934)
//!
//! The "Code Atlas" workflow (`.github/workflows/atlas.yml`) fails on every
//! push to `main`. Its "Render DOT diagrams" step iterates a fixed layer list
//! and invokes Graphviz `dot` on `docs/atlas/<layer>/<layer>.dot` for each
//! entry. The list originally included `user-journeys`, which is a
//! **Mermaid-only** layer with NO `.dot` file:
//!
//!   dot: can't open docs/atlas/user-journeys/user-journeys.dot
//!
//! `dot` exits 2, failing the step and the whole workflow.
//!
//! The fix removes `user-journeys` from the DOT render loop so `dot` is only
//! invoked on the 7 layers that each have a matching `.dot` file:
//! repo-surface, ast-lsp-bindings, compile-deps, runtime-topology,
//! api-contracts, data-flow, service-components.
//!
//! ## Contract enforced by these tests
//!
//! 1. `atlas.yml` exists and contains the "Render DOT diagrams" step.
//! 2. Every layer named in the DOT render loop has a corresponding
//!    `docs/atlas/<layer>/<layer>.dot` file on disk (the exact invariant the
//!    `dot` command relies on). This is the assertion that FAILED (red)
//!    before the fix because `user-journeys` had no `.dot`.
//! 3. `user-journeys` is NOT present in the DOT render loop (regression guard
//!    for #934).
//! 4. `user-journeys` is genuinely Mermaid-only: no `.dot` file exists for it,
//!    but at least one `.mmd` file does — documenting WHY it must be excluded.
//! 5. All 7 expected DOT layers are present in the loop (no accidental
//!    over-removal).
//!
//! These tests FAIL (red) against the pre-fix `atlas.yml` (which lists
//! `user-journeys` with no `.dot`) and PASS (green) once `user-journeys` is
//! removed from the loop.
//!
//! ## Related
//!
//! - `.github/workflows/atlas.yml` — the workflow under test
//! - `docs/atlas/index.md` — documents the dual-format vs Mermaid-only layers

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Workspace root, derived from this test binary's manifest dir.
/// For integration tests registered under `bins/amplihack`, CARGO_MANIFEST_DIR
/// points at `bins/amplihack`; walk up two levels to the workspace root.
fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

/// Locate `.github/workflows/atlas.yml`.
fn atlas_yml_path() -> PathBuf {
    let mut path = workspace_root();
    path.push(".github");
    path.push("workflows");
    path.push("atlas.yml");
    path
}

/// Read atlas.yml, panicking with a clear message if the file is missing.
fn read_atlas_yml() -> String {
    let path = atlas_yml_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "atlas.yml not found at {path:?}\n\
             Ensure .github/workflows/atlas.yml exists and the workspace is intact.\n\
             Error: {e}"
        )
    })
}

/// Extract the shell body of the "Render DOT diagrams" step.
///
/// Returns everything from the `name: Render DOT diagrams` marker up to (but
/// not including) the next step (a line whose trimmed form starts with
/// `- name:`) or end of file. This lets us inspect exactly which layers the
/// loop iterates without being confused by other steps.
fn render_dot_step_body(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let marker_idx = lines
        .iter()
        .position(|line| line.contains("Render DOT diagrams"))
        .unwrap_or_else(|| {
            panic!(
                "FAIL: atlas.yml does not contain a step named 'Render DOT diagrams'.\n\
                 The Code Atlas workflow must have this step to render Graphviz DOT layers.\n\
                 \n\
                 atlas.yml contents:\n{content}"
            )
        });

    // Collect from the marker line up to (but not including) the next step.
    lines[marker_idx..]
        .iter()
        .enumerate()
        .take_while(|(offset, line)| *offset == 0 || !line.trim_start().starts_with("- name:"))
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The layers that legitimately have a `<layer>.dot` file and must be rendered.
const EXPECTED_DOT_LAYERS: &[&str] = &[
    "repo-surface",
    "ast-lsp-bindings",
    "compile-deps",
    "runtime-topology",
    "api-contracts",
    "data-flow",
    "service-components",
];

/// The Mermaid-only layer that must NOT appear in the DOT render loop.
const MERMAID_ONLY_LAYER: &str = "user-journeys";

/// Path to `docs/atlas/<layer>/<layer>.dot`.
fn layer_dot_path(layer: &str) -> PathBuf {
    let mut path = workspace_root();
    path.push("docs");
    path.push("atlas");
    path.push(layer);
    path.push(format!("{layer}.dot"));
    path
}

/// Determine which layers the render loop actually iterates, by scanning the
/// step body for each candidate layer token. We check the known universe of
/// layers (the 7 DOT layers plus the Mermaid-only one) rather than parsing the
/// shell, which keeps the test robust to formatting/line-continuation changes.
fn layers_in_render_loop(step_body: &str) -> Vec<String> {
    let mut found = Vec::new();
    for layer in EXPECTED_DOT_LAYERS
        .iter()
        .copied()
        .chain(std::iter::once(MERMAID_ONLY_LAYER))
    {
        // Match the bare layer token as it appears in the `for layer in ...`
        // list. Require a surrounding non-identifier boundary so that, e.g.,
        // matching "data-flow" does not also match a hypothetical
        // "data-flow-extra".
        if contains_token(step_body, layer) {
            found.push(layer.to_string());
        }
    }
    found
}

/// True if `haystack` contains `token` bounded by characters that are not part
/// of a layer identifier (`[A-Za-z0-9-]`).
fn contains_token(haystack: &str, token: &str) -> bool {
    let bytes = haystack.as_bytes();
    let tbytes = token.as_bytes();
    let mut i = 0;
    while let Some(pos) = haystack[i..].find(token) {
        let start = i + pos;
        let end = start + tbytes.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        i = start + 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

// ---------------------------------------------------------------------------
// TEST-1: The atlas workflow file exists
// ---------------------------------------------------------------------------

#[test]
fn atlas_yml_file_is_present() {
    let path = atlas_yml_path();
    assert!(
        path.exists(),
        "FAIL: .github/workflows/atlas.yml not found at {path:?}.\n\
         This file must exist for the Code Atlas workflow to run."
    );
}

// ---------------------------------------------------------------------------
// TEST-2: The "Render DOT diagrams" step exists
// ---------------------------------------------------------------------------

#[test]
fn atlas_has_render_dot_step() {
    let content = read_atlas_yml();
    assert!(
        content.contains("Render DOT diagrams"),
        "FAIL: atlas.yml does not contain a step named 'Render DOT diagrams'.\n\
         \n\
         atlas.yml contents:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// TEST-3 (core #934 invariant): every layer in the DOT loop has a .dot file
// ---------------------------------------------------------------------------

/// This is the exact invariant `dot` relies on. Before the fix, the loop
/// included `user-journeys`, which has no `.dot`, so `dot` exited 2. This test
/// FAILS (red) on the pre-fix workflow and PASSES (green) after `user-journeys`
/// is removed from the loop.
#[test]
fn every_layer_in_render_loop_has_a_dot_file() {
    let content = read_atlas_yml();
    let step = render_dot_step_body(&content);
    let layers = layers_in_render_loop(&step);

    assert!(
        !layers.is_empty(),
        "FAIL: could not identify any layers in the 'Render DOT diagrams' loop.\n\
         Step body:\n{step}"
    );

    let mut missing = Vec::new();
    for layer in &layers {
        let dot = layer_dot_path(layer);
        if !dot.exists() {
            missing.push(format!(
                "  - {layer}: expected file {dot:?} (does not exist)"
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "FAIL (issue #934): the 'Render DOT diagrams' loop references layers \
         that have NO `<layer>.dot` file. `dot` will exit 2 on these:\n{}\n\
         \n\
         Remove Mermaid-only layers from the DOT render loop in atlas.yml so \
         `dot` is only invoked on layers that ship a matching `.dot` file.",
        missing.join("\n")
    );
}

// ---------------------------------------------------------------------------
// TEST-4 (regression guard): user-journeys is NOT in the DOT loop
// ---------------------------------------------------------------------------

#[test]
fn user_journeys_not_in_dot_render_loop() {
    let content = read_atlas_yml();
    let step = render_dot_step_body(&content);

    assert!(
        !contains_token(&step, MERMAID_ONLY_LAYER),
        "FAIL (issue #934 regression): '{MERMAID_ONLY_LAYER}' appears in the \
         'Render DOT diagrams' loop, but it is a Mermaid-only layer with no \
         `.dot` file. This is the exact cause of the CI failure:\n\
         \n\
           dot: can't open docs/atlas/{MERMAID_ONLY_LAYER}/{MERMAID_ONLY_LAYER}.dot\n\
         \n\
         Remove '{MERMAID_ONLY_LAYER}' from the DOT render loop.\n\
         \n\
         Render DOT step body:\n{step}"
    );
}

// ---------------------------------------------------------------------------
// TEST-5 (rationale guard): user-journeys really is Mermaid-only
// ---------------------------------------------------------------------------

/// Documents WHY `user-journeys` must be excluded: it has no `.dot` file but
/// does ship Mermaid (`.mmd`) sources. If someone later adds a
/// `user-journeys.dot`, this test surfaces that so the exclusion can be
/// revisited deliberately.
#[test]
fn user_journeys_is_mermaid_only() {
    let dot = layer_dot_path(MERMAID_ONLY_LAYER);
    assert!(
        !dot.exists(),
        "UNEXPECTED: {dot:?} now exists. '{MERMAID_ONLY_LAYER}' was Mermaid-only \
         (the basis for excluding it from the DOT render loop in issue #934). \
         If a real DOT source now exists, revisit atlas.yml to render it, and \
         update this test."
    );

    let mut dir = workspace_root();
    dir.push("docs");
    dir.push("atlas");
    dir.push(MERMAID_ONLY_LAYER);

    let has_mmd = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("FAIL: cannot read {dir:?}: {e}"))
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext == "mmd")
                .unwrap_or(false)
        });

    assert!(
        has_mmd,
        "FAIL: expected at least one `.mmd` file under {dir:?} to confirm \
         '{MERMAID_ONLY_LAYER}' is a Mermaid layer. If the layer was removed \
         entirely, this test's premise no longer holds and should be updated."
    );
}

// ---------------------------------------------------------------------------
// TEST-6 (completeness guard): all 7 expected DOT layers remain in the loop
// ---------------------------------------------------------------------------

/// Guards against over-removal: the fix must strip only `user-journeys`, not
/// any of the 7 layers that legitimately have `.dot` files.
#[test]
fn all_expected_dot_layers_present_in_loop() {
    let content = read_atlas_yml();
    let step = render_dot_step_body(&content);

    let mut absent = Vec::new();
    for layer in EXPECTED_DOT_LAYERS {
        if !contains_token(&step, layer) {
            absent.push(*layer);
        }
    }

    assert!(
        absent.is_empty(),
        "FAIL: the following expected DOT layers are missing from the \
         'Render DOT diagrams' loop: {absent:?}.\n\
         Each of these ships a `docs/atlas/<layer>/<layer>.dot` file and must \
         be rendered. Do not remove them.\n\
         \n\
         Render DOT step body:\n{step}"
    );
}
