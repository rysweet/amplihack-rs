//! Conservative issue linkage shared by commit and PR publication.
use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum IssueLinkMode {
    Auto,
    Refs,
    Closes,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Classification {
    #[value(name = "Closes")]
    Substantive,
    #[value(name = "Refs")]
    NonSubstantive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKeyword {
    Refs,
    Closes,
}

/// Closing requires a successful substantive classification and caller consent.
pub fn select_issue_keyword(
    mode: IssueLinkMode,
    classification: Option<Classification>,
) -> IssueKeyword {
    match (mode, classification) {
        (IssueLinkMode::Auto | IssueLinkMode::Closes, Some(Classification::Substantive)) => {
            IssueKeyword::Closes
        }
        _ => IssueKeyword::Refs,
    }
}

#[derive(Args, Debug)]
pub struct IssueKeywordArgs {
    #[arg(long, value_enum, default_value = "auto")]
    mode: IssueLinkMode,
    /// Supply only a successful classifier's exact verdict; omit on failure.
    #[arg(long, value_enum)]
    classification: Option<Classification>,
}

pub fn run(args: IssueKeywordArgs) {
    println!(
        "{}",
        match select_issue_keyword(args.mode, args.classification) {
            IssueKeyword::Refs => "Refs",
            IssueKeyword::Closes => "Closes",
        }
    );
}
