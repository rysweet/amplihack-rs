use super::*;

const NOISY_REGRESSIONS: &[&str] = &[
    "session_start.sh ❌",
    "post_tool_use.sh ❌",
    "pre_tool_use.sh ❌",
    "profile_management",
    "Skipping symlink",
];

#[test]
fn clean_install_update_output_contract_accepts_transition_message_without_noisy_regressions() {
    let output = "\
🚀 Starting amplihack installation...
  ℹ️  Missing assets will self-heal on next invocation
     • tools/xpia/hooks/session_start.sh
     • tools/xpia/hooks/post_tool_use.sh
     • tools/xpia/hooks/pre_tool_use.sh
✅ Amplihack installation completed successfully!
";

    settings::assert_no_noisy_install_update_regressions(output)
        .expect("post-update transition output should be accepted");
}

#[test]
fn clean_install_update_output_contract_rejects_known_noisy_regressions() {
    for noisy in NOISY_REGRESSIONS {
        let output = format!("install output\n  ⚠️  {noisy}\n");

        let err = settings::assert_no_noisy_install_update_regressions(&output)
            .expect_err("known noisy regression must be rejected");

        assert!(
            err.to_string().contains(noisy),
            "error must identify the offending regression string `{noisy}`, got: {err}"
        );
    }
}

#[test]
fn post_update_xpia_shell_asset_gaps_are_not_rendered_as_missing_hook_failures() {
    let (output, still_missing) = settings::render_framework_asset_verification(
        [
            "tools/xpia/hooks/session_start.sh",
            "tools/xpia/hooks/post_tool_use.sh",
            "tools/xpia/hooks/pre_tool_use.sh",
        ]
        .map(String::from)
        .to_vec(),
        true,
    );
    assert!(
        still_missing.is_empty(),
        "post-update transitional shell gaps must not remain fatal: {still_missing:?}"
    );

    settings::assert_no_noisy_install_update_regressions(&output)
        .expect("rendered post-update verification output must not contain noisy regressions");
    assert!(
        !output.contains("❌ Missing required framework assets"),
        "old-version post-update transition must not render missing XPIA shell assets as failures:\n{output}"
    );
}

/// The entries `render_framework_asset_verification` actually receives.
///
/// `missing_framework_paths` renders every gap as
/// `"{relative} (expected at {absolute})"`. The two tests above feed bare
/// relative paths, which is a shape production never produces — and that is
/// precisely why the tolerance predicate could stop matching anything without
/// a single test noticing. Feed the real shape.
fn as_missing_entry(relative: &str) -> String {
    format!("{relative} (expected at /home/u/.amplihack/.claude/{relative})")
}

#[test]
fn tolerated_gaps_are_recognised_in_the_rendered_entry_form_production_uses() {
    let (output, still_missing) = settings::render_framework_asset_verification(
        vec![as_missing_entry("tools/xpia/hooks/session_start.sh")],
        true,
    );
    assert!(
        still_missing.is_empty(),
        "this gap is tolerated by name; matched against the rendered entry it \
         was fatal, which is how a stale source bundle turned one new file into \
         `self-heal failed`: {still_missing:?}\n{output}"
    );
    assert!(
        !output.contains("❌ Missing required framework assets"),
        "tolerated gaps must not render as failures:\n{output}"
    );
}

#[test]
fn a_genuinely_required_asset_is_still_fatal_in_the_rendered_entry_form() {
    // The other half: loosening the match must not make everything tolerated.
    let (output, still_missing) = settings::render_framework_asset_verification(
        vec![as_missing_entry("tools/statusline.sh")],
        true,
    );
    assert_eq!(
        still_missing.len(),
        1,
        "a required asset must remain fatal:\n{output}"
    );
}
