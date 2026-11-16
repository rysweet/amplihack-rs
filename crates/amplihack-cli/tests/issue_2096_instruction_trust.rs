//! Reviewed instruction files remain exact data; launch recipes stay outside
//! the reviewed repository. Actual CLI discovery requires adapter acceptance.
use super::*;

#[test]
fn pr_instruction_files_are_preserved_as_review_data_and_never_supply_launch_recipes() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let inputs = [
        ("AGENTS.md", "PR-controlled agent instructions\n"),
        ("CLAUDE.md", "PR-controlled reviewer instructions\n"),
        (".claude/settings.json", "{\"hooks\":{}}\n"),
        ("nested/AGENTS.md", "nested PR-controlled instructions\n"),
        (
            "amplifier-bundle/recipes/autodrive-crusty-round.yaml",
            "name: malicious-round\nsteps: []\n",
        ),
    ];
    for (relative, contents) in inputs {
        let file = pr.join(relative);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, contents).unwrap();
    }
    f.git_in(&pr, &["add", "-A"]);
    f.git_in(&pr, &["commit", "-qm", "PR-controlled instruction data"]);
    let review = private_review(&f, &pr);
    for (relative, contents) in inputs {
        assert_eq!(
            fs::read(review.join(relative)).unwrap(),
            contents.as_bytes(),
            "review provisioning must not replace reviewed instructions with trusted copies"
        );
    }
    let output = preflight(&f, &pr, &review);
    succeeds(&output);
    let lines = String::from_utf8(output.stdout).unwrap();
    let recipe = Path::new(lines.lines().nth(1).expect("trusted launch recipe"));
    assert!(!recipe.starts_with(&pr) && !recipe.starts_with(&review));
    assert_ne!(fs::read(recipe).unwrap(), inputs[4].1.as_bytes());
}
