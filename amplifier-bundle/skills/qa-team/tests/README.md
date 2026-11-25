# Filesystem QA scenarios

Exercise an installer or configuration command through its public executable.
Use the files it publishes as the evidence. These scenarios also apply when
comparing a full installation with a smaller default selection.

## Fresh installation

Start with an empty temporary home. Install into it, then enumerate the skill
folders and agent files that each supported tool can discover. Compare the
inventory with the requested selection. Check that definitions omitted from
the tool's lists remain readable through the complete framework and index.

## Updating a full installation

Install the full selection first. Edit one installer-owned skill and add a
separate user-owned skill. Change to the default selection and update. Check
that unedited omitted entries disappear from the tool's lists, the edited
entry remains, and the user-owned entry is unchanged. Check the retained
framework definitions and any migration record separately.

## Persistent extensions

Snapshot configuration bytes and tool-readable assets. Enable an omitted
entry through the public command, then check its published definition. Repeat
the command and check that the same selection remains. Disable the entry and
compare configuration and published assets with the original snapshot.
Internal transaction records and migration notices need their own checks.

## Restoring full lists

Run the documented restoring command by itself. Check the full inventory for
each previously installed tool, then repeat the command to check convergence.
Check that no additional tool was installed. An unsuccessful command must
return a failing exit status and provide enough output to diagnose the failure.

Implement these scenarios with native integration tests in Rust projects.
Keep assertions about behaviour and file content, rather than explanatory
wording. Read `../scripts/README.md` for execution guidance.
