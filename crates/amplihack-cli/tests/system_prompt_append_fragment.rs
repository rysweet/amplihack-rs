//! Asset and install-plumbing contracts for issue #1265 Option 3.
//!
//! The decision function and the argv wiring are covered by the in-crate unit
//! tests (`src/commands/launch/tests_system_prompt_append.rs`). This file
//! covers the two things that only exist outside the crate's code: the shipped
//! fragment itself, and the install-manifest entry that is what actually
//! delivers it to an existing install.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn fragment_source_path() -> PathBuf {
    repo_root().join("amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md")
}

fn fragment_text() -> String {
    let path = fragment_source_path();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the shipped fragment must exist at {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// The shipped fragment
// ---------------------------------------------------------------------------

#[test]
fn fragment_exists_and_is_not_empty() {
    assert!(
        !fragment_text().trim().is_empty(),
        "the fragment is the entire feature — an empty one delivers nothing"
    );
}

#[test]
fn fragment_is_bounded_to_twenty_five_lines() {
    // It is read into EVERY session, so every line costs context on every
    // launch. A long fragment is a recurring tax, and a fragment nobody wants
    // to pay for gets disabled.
    let text = fragment_text();
    let lines = text.lines().count();
    assert!(
        lines <= 25,
        "the fragment must stay at or under 25 lines, got {lines}"
    );
}

#[test]
fn fragment_names_amplihack_as_the_operator() {
    // The agent needs to know whose contract it is under before it can be
    // asked to prefer that contract over a generic instruction.
    let text = fragment_text().to_lowercase();
    assert!(
        text.contains("amplihack"),
        "the fragment must name amplihack:\n{text}"
    );
}

#[test]
fn fragment_names_the_authoritative_channels() {
    // The outranked channels inherit this one's rank only if this one names
    // them.
    let text = fragment_text();
    assert!(
        text.contains("UserPromptSubmit"),
        "the fragment must name the hook channel it makes authoritative:\n{text}"
    );
    assert!(
        text.contains("CLAUDE.md"),
        "the fragment must name CLAUDE.md:\n{text}"
    );
}

#[test]
fn fragment_quotes_the_known_contrary_instructions_verbatim() {
    // An override that says "ignore anything that conflicts" is a tone contest
    // against a specific, concrete instruction — and specificity usually wins.
    // Quoting the exact strings is what makes the override unmistakable rather
    // than a matter of interpretation. These two are the offenders observed in
    // a live incident.
    let text = fragment_text();
    for offender in [
        "Do not call the AgentTool unless the user requested it",
        "Do not use workflows or deep-research unless the user requested it",
    ] {
        assert!(
            text.contains(offender),
            "the fragment must quote {offender:?} verbatim:\n{text}"
        );
    }
}

#[test]
fn fragment_resolves_the_unless_the_user_requested_it_clause() {
    // The contrary instructions are not argued with — their own precondition
    // is satisfied. Launching through amplihack IS the request.
    let text = fragment_text().to_lowercase();
    assert!(
        text.contains("supersede") || text.contains("override") || text.contains("do not apply"),
        "the fragment must state that it outranks a conflicting earlier \
         instruction:\n{text}"
    );
    assert!(
        text.contains("is the user's request")
            || text.contains("is the user request")
            || (text.contains("launching through amplihack") && text.contains("request")),
        "the fragment must state that launching through amplihack IS the \
         user's request to use its router:\n{text}"
    );
}

#[test]
fn fragment_carries_the_world_visible_warning() {
    // The contents are passed on the command line and are visible in `ps` to
    // every user on the host. The rule is needed precisely BECAUSE the file
    // looks like a config file, which is where such things normally go.
    let text = fragment_text().to_lowercase();
    assert!(
        text.contains("process table") || text.contains("visible"),
        "the fragment's own header must warn that its bytes are world-visible \
         in the process table:\n{text}"
    );
    assert!(
        text.contains("never") && (text.contains("credential") || text.contains("secret")),
        "the fragment's header must say plainly: never put credentials or \
         secrets in this file:\n{text}"
    );
}

#[test]
fn fragment_contains_no_secrets_shaped_content() {
    // Cheap standing guard on the file it warns about.
    let text = fragment_text();
    for smell in ["ghp_", "sk-ant-", "AKIA", "BEGIN PRIVATE KEY", "password="] {
        assert!(
            !text.contains(smell),
            "the fragment must never carry credential-shaped content ({smell})"
        );
    }
}

// ---------------------------------------------------------------------------
// Delivery: the install-manifest entry is what reaches existing installs
// ---------------------------------------------------------------------------

fn install_types_src() -> String {
    std::fs::read_to_string(repo_root().join("crates/amplihack-cli/src/commands/install/types.rs"))
        .expect("install/types.rs")
}

fn essential_files_body(src: &str) -> String {
    let start = src
        .find("fn essential_files(")
        .expect("essential_files must exist");
    let rest = &src[start..];
    let mut depth = 0;
    for (i, ch) in rest.char_indices() {
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return rest[..=i].to_string();
            }
        }
    }
    rest.to_string()
}

#[test]
fn fragment_is_never_registered_in_essential_files() {
    // This assertion used to run the other way, and inverting it is the point.
    //
    // Listing the fragment here was how the feature reached an existing
    // install: `missing_framework_paths` reports the gap, and no install in the
    // wild carries a newly-added file, so the restage fires for every user on
    // the first launch after upgrade. `ensure_framework_installed` resolves its
    // source with `find_bundled_framework_root`, whose second step walks UP
    // from `current_dir()` and accepts any ancestor with an `amplifier-bundle/`
    // passing a *shape* check — then copies `context/`, `agents/`, `skills/`
    // and `tools/amplihack/*.sh` out of it into `$HOME/.amplihack/.claude/`.
    //
    // For a file whose bytes are then handed to the agent at system-prompt
    // privilege — under this fragment's own "supersedes any earlier
    // instruction" framing — that made `git clone <fork> && cd <fork> &&
    // amplihack claude` a permanent, host-wide injection affecting every later
    // session in every other repo.
    //
    // The fragment is `include_str!`d into the binary instead, so it reaches
    // every install with no restage at all. Re-adding the listing is the single
    // edit that re-arms the chain.
    let body = essential_files_body(&install_types_src());
    assert!(
        !body.contains("SYSTEM_PROMPT_APPEND"),
        "the fragment must NOT be an essential file — it is compiled into the \
         binary, and listing it here arms a cwd-sourced restage of $HOME:\n{body}"
    );
}

#[test]
fn the_fragment_is_compiled_in_rather_than_read_from_disk() {
    // The other half of the same contract, by shape rather than by filename, so
    // it also catches a reintroduced reader that spells the path differently.
    let src = std::fs::read_to_string(
        repo_root().join("crates/amplihack-cli/src/commands/launch/system_prompt_append.rs"),
    )
    .expect("the module must exist");
    assert!(
        src.contains("include_str!"),
        "the fragment must be compiled in:\n{src}"
    );
    for reader in ["File::open", "read_to_string", "fs::read", "metadata("] {
        assert!(
            !src.contains(reader),
            "{reader} reintroduces a runtime read of the fragment; the whole \
             point is that there is no file to trust, no size to cap and no \
             restage to arm"
        );
    }
}

#[test]
fn fragment_is_absent_from_the_legacy_arm() {
    // Subsumed by `fragment_is_never_registered_in_essential_files` now that
    // neither arm lists it, and kept as an independent control: the legacy arm
    // was always the one that would have tripped the documented re-install loop
    // (the bundle is the only layout that ships the file), so a future edit
    // that re-adds the entry to only one arm should still be caught here.
    let body = essential_files_body(&install_types_src());
    let legacy_arm = body
        .split("SourceLayout::LegacyClaude")
        .nth(1)
        .expect("legacy arm must exist");
    assert!(
        !legacy_arm.contains("SYSTEM_PROMPT_APPEND.md"),
        "the fragment must NOT be required by the legacy layout:\n{legacy_arm}"
    );
}

#[test]
fn no_parallel_install_mechanism_was_invented() {
    // The requirement was to use whatever already installs amplihack's other
    // context files. `copy_dir_recursive` over `context/` plus one
    // essential_files entry is the whole change; a bespoke copy step for this
    // one file would be a second mechanism to keep in sync.
    let src = install_types_src();
    let occurrences = src.matches("SYSTEM_PROMPT_APPEND").count();
    assert!(
        occurrences <= 1,
        "expected exactly one manifest entry, found {occurrences} references \
         — that smells like a parallel install path"
    );
}

// ---------------------------------------------------------------------------
// Scope gate: Option 3 only
// ---------------------------------------------------------------------------

#[test]
fn the_launcher_path_form_uses_the_file_flag() {
    // `LauncherConfig::append_system_prompt` is a PathBuf and
    // `build_claude_command` passed it to `--append-system-prompt`, which takes
    // a prompt string — so it was handing claude a path and calling it a
    // prompt. Wiring this feature while leaving a sibling emitter that provably
    // emits garbage is not defensible.
    let src =
        std::fs::read_to_string(repo_root().join("crates/amplihack-launcher/src/launcher_core.rs"))
            .expect("launcher_core.rs");
    assert!(
        src.contains("--append-system-prompt-file"),
        "the path-shaped sibling must emit the -file form"
    );
}

#[test]
fn the_flag_matrix_doc_comment_does_not_claim_the_flag_takes_a_path() {
    let src =
        std::fs::read_to_string(repo_root().join("crates/amplihack-launcher/src/flag_matrix.rs"))
            .expect("flag_matrix.rs");
    let start = src
        .find("supports_append_prompt")
        .expect("field must exist");
    let doc = &src[start.saturating_sub(200)..start];
    assert!(
        !doc.contains("--append-system-prompt <path>"),
        "the flag takes a prompt string, not a path — the doc comment is wrong:\n{doc}"
    );
}

#[test]
fn option_4_and_option_5_surfaces_are_absent() {
    // Scope is Option 3 ONLY. Options 4 and 5 from issue #1265 are out.
    let root = repo_root();
    for forbidden_path in [
        "crates/amplihack-cli/src/commands/launch/system_prompt_settings.rs",
        "crates/amplihack-cli/src/commands/launch/system_prompt_output_style.rs",
    ] {
        assert!(
            !root.join(forbidden_path).exists(),
            "{forbidden_path} belongs to an option that is out of scope"
        );
    }
}

#[test]
fn the_feature_is_documented_and_linked_from_the_docs_index() {
    let root = repo_root();
    assert!(
        root.join("docs/SYSTEM_PROMPT_APPEND.md").exists(),
        "a future maintainer must not have to read the issue to understand why \
         this exists"
    );
    let index = std::fs::read_to_string(root.join("docs/index.md")).expect("docs/index.md");
    assert!(
        index.contains("SYSTEM_PROMPT_APPEND.md"),
        "an unlinked doc is an unfindable doc"
    );
}

#[test]
fn the_docs_lead_with_the_delivery_channel_argument() {
    // The point that has to survive: hooks and CLAUDE.md are STRUCTURALLY
    // outranked by the base system prompt, so no amount of rewording fixes it.
    // Someone who reads this as a wording problem will "fix" it by editing
    // CLAUDE.md and wonder why nothing changes.
    let doc = std::fs::read_to_string(repo_root().join("docs/SYSTEM_PROMPT_APPEND.md"))
        .expect("docs/SYSTEM_PROMPT_APPEND.md");
    let lower = doc.to_lowercase();
    assert!(
        lower.contains("outranked") || lower.contains("delivery channel"),
        "the doc must lead with the delivery-channel argument"
    );
    assert!(
        lower.contains("userpromptsubmit") && lower.contains("claude.md"),
        "the doc must name the two outranked channels"
    );
}

/// `docs/SYSTEM_PROMPT_APPEND.md` reproduces the shipped fragment in a fenced
/// block so the reader can see the exact bytes the agent receives. Nothing
/// linked the two: the doc copy drifted from the shipped file by one bold span
/// (`**is** the user's request` vs `**is the user's request**`) and no build
/// step, test, or review caught it — a page whose whole claim is "these are the
/// bytes" was quoting bytes that were never sent.
///
/// Prose *about* the fragment is free to differ; this pins only the quoted
/// block. It is anchored on the fragment's own first line rather than a line
/// number, so editing the surrounding prose cannot silently un-pin it.
#[test]
fn the_documented_fragment_is_the_shipped_fragment_verbatim() {
    let shipped_path = fragment_source_path();
    let doc_path = repo_root().join("docs/SYSTEM_PROMPT_APPEND.md");

    let shipped = std::fs::read_to_string(&shipped_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", shipped_path.display()));
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", doc_path.display()));

    let anchor = "<!-- Compiled into the binary at build time";
    assert!(
        shipped.starts_with(anchor),
        "the shipped fragment no longer starts with the anchor this test keys \
         on; update the anchor here and in {}",
        doc_path.display()
    );

    let quoted = doc
        .split("```markdown\n")
        .find(|block| block.starts_with(anchor))
        .and_then(|block| block.split("\n```").next())
        .unwrap_or_else(|| {
            panic!(
                "{} no longer contains a ```markdown block quoting the fragment \
                 (expected one starting with {anchor:?})",
                doc_path.display()
            )
        });

    assert_eq!(
        quoted.trim_end(),
        shipped.trim_end(),
        "the fragment quoted in {} has drifted from the shipped {}. The doc \
         claims to show the exact bytes the agent receives, so the quoted block \
         must be copied verbatim — update the doc, not this test.",
        doc_path.display(),
        shipped_path.display()
    );
}
