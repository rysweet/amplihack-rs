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
  ℹ️  Missing transitional XPIA shell assets will self-heal on next invocation
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
    let output = settings::render_framework_asset_verification_for_test(
        &[
            "tools/xpia/hooks/session_start.sh",
            "tools/xpia/hooks/post_tool_use.sh",
            "tools/xpia/hooks/pre_tool_use.sh",
        ],
        settings::FrameworkAssetVerificationMode::PostUpdateInstall,
    )
    .expect("post-update transitional shell gaps should render as informational");

    settings::assert_no_noisy_install_update_regressions(&output)
        .expect("rendered post-update verification output must not contain noisy regressions");
    assert!(
        !output.contains("❌ Missing required framework assets"),
        "old-version post-update transition must not render missing XPIA shell assets as failures:\n{output}"
    );
}
