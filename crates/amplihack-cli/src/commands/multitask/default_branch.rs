pub(super) fn parse_from_ls_remote(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find(|line| line.starts_with("ref: refs/heads/"))
        .and_then(|line| line.strip_prefix("ref: refs/heads/"))
        .and_then(|line| line.split('\t').next())
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(ToOwned::to_owned)
}
