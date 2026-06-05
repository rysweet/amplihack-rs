use amplihack_cli::install_output_contract::assert_no_noisy_install_update_regressions;

#[test]
fn install_update_smoke_output_rejects_missing_xpia_hook_error_lines() {
    for hook in ["session_start.sh", "post_tool_use.sh", "pre_tool_use.sh"] {
        let output = format!("🔍 Verifying staged framework assets:\n  ❌ {hook}\n");

        let err = assert_no_noisy_install_update_regressions(&output)
            .expect_err("missing XPIA hook error output must fail smoke assertion");

        assert!(
            err.to_string().contains(hook),
            "smoke assertion should name the noisy hook line, got: {err}"
        );
    }
}

#[test]
fn install_update_smoke_output_rejects_profile_management_and_safe_symlink_noise() {
    for noisy in [
        "profile_management",
        "Skipping symlink: amplifier-bundle/skills/docx/ooxml",
    ] {
        let output = format!("install output\n  ⚠️  {noisy}\n");

        let err = assert_no_noisy_install_update_regressions(&output)
            .expect_err("known noisy output regression must fail smoke assertion");

        assert!(
            err.to_string().contains(noisy),
            "smoke assertion should name `{noisy}`, got: {err}"
        );
    }
}

#[test]
fn install_update_smoke_output_accepts_clean_transition_guidance() {
    let output = "\
Updated amplihack: 0.9.70 -> 0.9.71
Restart amplihack to use the new version.
  ℹ️  Missing transitional XPIA shell assets will self-heal on next invocation
     • tools/xpia/hooks/session_start.sh
";

    assert_no_noisy_install_update_regressions(output)
        .expect("clean old-version transition guidance should pass smoke assertion");
}
