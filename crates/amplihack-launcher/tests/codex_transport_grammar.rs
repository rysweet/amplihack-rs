//! Transport follows native image arity and caller-owned prompt syntax.
use amplihack_launcher::{
    flag_matrix::AgentBinary,
    prompt_delivery::{DeliveredCommand, build_tool_command_with_prompt_delivery},
};
use amplihack_utils::prompt_delivery::{DeliveryMode, PromptDelivery};
use std::path::Path;

fn build(args: &[&str], prompt: &str) -> std::io::Result<DeliveredCommand> {
    build_tool_command_with_prompt_delivery(
        AgentBinary::Codex,
        Path::new("."),
        &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        prompt,
        PromptDelivery::Auto,
    )
}
fn argv(d: &DeliveredCommand) -> Vec<String> {
    d.command
        .get_args()
        .map(|a| a.to_str().unwrap().to_owned())
        .collect()
}
#[test]
fn separate_images_consume_command_words_but_options_end_the_values() {
    for flag in ["--image", "-i"] {
        let args = [flag, "a.png", "exec", "resume"];
        let d = build(&args, "task").unwrap();
        assert_eq!(d.selected_mode, DeliveryMode::Argv);
        assert_eq!(argv(&d), [flag, "a.png", "exec", "resume", "--", "task"]);
        assert!(d.stdin_payload.is_none());
        assert!(build(&args, &"λ".repeat(100_000)).is_err());
        for suffix in [
            vec!["exec"],
            vec!["e"],
            vec!["exec", "resume", "--last"],
            vec!["exec", "resume", "session"],
        ] {
            let mut args = vec![flag, "a.png", "b.png", "--model", "caller"];
            args.extend(suffix);
            for prompt in ["private task".to_owned(), "λ\n".repeat(100_000)] {
                let d = build(&args, &prompt).unwrap();
                let mut expected = args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
                expected.push("-".into());
                assert_eq!(argv(&d), expected);
                assert_eq!(d.selected_mode, DeliveryMode::Stdin);
                assert_eq!(d.stdin_payload.as_deref(), Some(prompt.as_bytes()));
                assert!(!d.command.get_args().any(|a| a == prompt.as_str()));
            }
        }
    }
}
#[test]
fn inline_and_attached_images_leave_exec_and_resume_in_command_position() {
    for flag in ["--image=a.png,b.png", "-ia.png,b.png", "-i=a.png,b.png"] {
        for suffix in [vec!["exec"], vec!["exec", "resume", "--last"]] {
            let mut args = vec![flag];
            args.extend(suffix);
            let d = build(&args, "task").unwrap();
            assert_eq!(d.selected_mode, DeliveryMode::Stdin);
            assert_eq!(d.stdin_payload.as_deref(), Some(b"task".as_slice()));
        }
    }
}
#[test]
fn images_respect_exec_resume_unary_and_interactive_resume_variadic_scopes() {
    let args = ["exec", "resume", "--image", "a.png", "session", "-"];
    let d = build(&args, "task").unwrap();
    assert_eq!(argv(&d), args);
    assert_eq!(d.stdin_payload.as_deref(), Some(b"task".as_slice()));
    let args = ["resume", "--image", "a.png", "exec", "--last"];
    let d = build(&args, "task").unwrap();
    assert_eq!(
        argv(&d),
        ["resume", "--image", "a.png", "exec", "--last", "--", "task"]
    );
    assert!(d.stdin_payload.is_none());
    let d = build(&["exec", "--image", "a.png", "resume"], "task").unwrap();
    assert_eq!(argv(&d), ["exec", "--image", "a.png", "resume", "--", "-"]);
}
#[test]
fn existing_delimiters_and_stdin_markers_are_preserved_once() {
    let d = build(&["--"], "--literal task").unwrap();
    assert_eq!(argv(&d), ["--", "--literal task"]);
    for args in [
        vec!["exec", "-"],
        vec!["exec", "--", "-"],
        vec!["exec", "resume", "--last", "-"],
        vec!["exec", "resume", "--last", "--", "-"],
        vec!["exec", "resume", "session", "--", "-"],
    ] {
        let d = build(&args, "task").unwrap();
        assert_eq!(argv(&d), args);
        assert_eq!(d.stdin_payload.as_deref(), Some(b"task".as_slice()));
    }
}
#[test]
fn caller_prompts_are_rejected_without_disclosing_their_contents() {
    for args in [
        vec!["private"],
        vec!["--", "private"],
        vec!["exec", "private"],
        vec!["exec", "--", "private"],
        vec!["exec", "-", "-"],
        vec!["resume", "session", "private"],
        vec!["exec", "resume", "--last", "private"],
        vec!["exec", "resume", "session", "private"],
    ] {
        let error = build(&args, "secret envelope").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!error.to_string().contains("private"));
        assert!(!error.to_string().contains("secret"));
    }
}
