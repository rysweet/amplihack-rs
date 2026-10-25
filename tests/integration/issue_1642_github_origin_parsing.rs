//! Issue #1642: GitHub origin parsing lives once, in
//! `amplifier-bundle/tools/workflow_github_repo.sh`, and every bundle caller
//! sources it. A non-GitHub origin whose userinfo before `@github.com/` holds
//! '/', '?', '#' or a backslash (git contacts the other host) is never read as
//! a github.com repository, and gh is never invoked for it. Every real origin
//! form still parses to `owner/repo`.
//!
//! Runs the real bundle scripts against private git repos and a stub `gh`;
//! needs bash, git and jq, never the network.
#[path = "issue_1642_github_origin_parsing/callers.rs"]
mod callers;
#[path = "issue_1642_github_origin_parsing/cases.rs"]
mod cases;
#[path = "issue_1642_github_origin_parsing/fixture.rs"]
mod fixture;
#[path = "issue_1642_github_origin_parsing/guard.rs"]
mod guard;
#[path = "issue_1642_github_origin_parsing/hostile.rs"]
mod hostile;
#[path = "issue_1642_github_origin_parsing/install.rs"]
mod install;
#[path = "issue_1642_github_origin_parsing/missing_helper.rs"]
mod missing_helper;
#[path = "issue_1642_github_origin_parsing/parser.rs"]
mod parser;
#[path = "issue_1642_github_origin_parsing/real_forms.rs"]
mod real_forms;
#[path = "issue_1642_github_origin_parsing/recipe.rs"]
mod recipe;
#[path = "issue_1642_github_origin_parsing/recipe_lookup.rs"]
mod recipe_lookup;
// SR2: the "helper not found" reason names `amplihack install` and lists
// every searched path with an exact marker.
#[path = "issue_1642_github_origin_parsing/recipe_lookup_message.rs"]
mod recipe_lookup_message;
