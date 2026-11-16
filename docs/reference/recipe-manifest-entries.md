---
title: Recipe manifest entries
description: What _recipe_manifest.json holds, how an entry is computed, how to regenerate and check the file, and what actually reads it.
doc_type: reference
last_updated: 2026-11-16
related_issues: [2528, 1966]
---

# Recipe manifest entries

`amplifier-bundle/recipes/_recipe_manifest.json` lists bundled recipes with a
short digest of each recipe's text. This page gives the file's shape, the rule
each entry follows, the commands that regenerate and check it, and the limits
of what reads it.

The manifest is a consistency record, not a security control. Nothing in
`amplihack install` or in a recipe run compares an entry with the recipe it
names; see [What reads the manifest](#what-reads-the-manifest).

## Contents

- [Location and shape](#location-and-shape)
- [The entry rule](#the-entry-rule)
- [The key set](#the-key-set)
- [Check the file](#check-the-file)
- [Regenerate the file](#regenerate-the-file)
- [What reads the manifest](#what-reads-the-manifest)
- [What keeps entries current](#what-keeps-entries-current)
- [Install-time errors](#install-time-errors)

## Location and shape

| Property | Value |
|----------|-------|
| Path in the repository | `amplifier-bundle/recipes/_recipe_manifest.json` |
| Path in an installed bundle | `<bundle>/recipes/_recipe_manifest.json` |
| Top-level JSON type | Object; any other type is refused at install time |
| Keys | Recipe names: the file name of a recipe in the same directory, without `.yaml` |
| Values | Strings: the first 16 lowercase hex characters of the recipe file's SHA-256 digest |
| Formatting | Keys sorted, two-space indent, one trailing newline (`json.dumps(entries, indent=2, sort_keys=True) + "\n"`) |

To see the shape, print the first lines of the file:

```bash
head -4 amplifier-bundle/recipes/_recipe_manifest.json
```

That is the opening brace and the first three entries in key order: one
`"<recipe name>": "<16 hex characters>"` pair per line, two spaces of indent.
The digests are not reproduced here, because every edit to a recipe changes
one; read them from the file.

The file is in the `recipes/` directory it describes, so it travels with the
bundle: `amplihack install` stages it with the other recipes, and a recipe
run's bundle snapshot holds a copy of it.

## The entry rule

An entry's value is the first 16 characters of the SHA-256 digest of the
recipe file's bytes:

```bash
sha256sum amplifier-bundle/recipes/auto-drive-light.yaml | cut -c1-16
```

To see the rule hold, compare that digest with the entry. On a current tree
this prints `entry for auto-drive-light matches`:

```bash
name=auto-drive-light
test "$(sha256sum "amplifier-bundle/recipes/$name.yaml" | cut -c1-16)" \
  = "$(python3 -I -c "import json;print(json.load(open('amplifier-bundle/recipes/_recipe_manifest.json'))['$name'])")" \
  && echo "entry for $name matches" || echo "entry for $name is stale"
```

The digest is over the file as stored, including comments and trailing
whitespace, so any edit to a recipe's text changes its entry. Sixteen
characters are kept because the value is a change marker, not a lookup key.

## The key set

The key set belongs to the file. It is not derived from the directory
listing, and it does not have to name every recipe: `recipes/` holds more
`*.yaml` files than the manifest has entries. A recipe added without an entry
stays without one until someone adds it on purpose. List the recipes that
have no entry:

```bash
python3 -I - <<'PY'
import json, pathlib
recipes = pathlib.Path("amplifier-bundle/recipes")
entries = json.loads((recipes / "_recipe_manifest.json").read_text())
for path in sorted(recipes.glob("*.yaml")):
    if path.stem not in entries:
        print(path.stem)
PY
```

Run it rather than trusting a list written down here: a merge that brings in
recipes, or a change that adds one, moves names on and off it.

Two consequences:

- regenerating the file recomputes the values of the keys that are already
  there and adds none, so a regeneration is never a silent inventory change.
  [Regenerate the file](#regenerate-the-file) uses the key-preserving form;
- adding a recipe to the manifest is a separate, deliberate edit: add the key
  with its digest.

Files that are not recipes are never keys. `_recipe_manifest.json` itself and
the shell tests under `recipes/tests/` have no entries.

## Check the file

Run from the repository root. It names each stale entry and exits 1 if there
is one; on a current tree it prints `all entries current` and exits 0:

```bash
python3 -I - <<'PY'
import hashlib, json, pathlib, sys
recipes = pathlib.Path("amplifier-bundle/recipes")
entries = json.loads((recipes / "_recipe_manifest.json").read_text())
stale = [
    name
    for name, value in entries.items()
    if hashlib.sha256((recipes / f"{name}.yaml").read_bytes()).hexdigest()[:16] != value
]
for name in stale:
    print(f"stale: {name}")
print("all entries current" if not stale else f"{len(stale)} stale")
sys.exit(1 if stale else 0)
PY
```

`python3 -I` is deliberate: it ignores the current directory and the user
site directory, so a file named `json.py` beside the command cannot be
imported instead of the standard library.

## Regenerate the file

Run from the repository root after editing any recipe, and after a merge that
changed recipe text:

```bash
python3 -I - <<'PY'
import hashlib, json, pathlib
recipes = pathlib.Path("amplifier-bundle/recipes")
manifest = recipes / "_recipe_manifest.json"
entries = json.loads(manifest.read_text())
entries = {
    name: hashlib.sha256((recipes / f"{name}.yaml").read_bytes()).hexdigest()[:16]
    for name in entries
}
manifest.write_text(json.dumps(entries, indent=2, sort_keys=True) + "\n")
print("manifest regenerated")
PY
```

On an up-to-date tree this rewrites the file byte for byte, so
`git diff --exit-code -- amplifier-bundle/recipes/_recipe_manifest.json` is
empty and the command is safe to run at any time. Hashes are computed from
the files in the repository, never from a downloaded or extracted bundle.

After a merge that touched both the manifest and the recipes, resolve the
manifest to the incoming key set and then regenerate, rather than merging
digests line by line: a digest carried over from either side is right only by
accident.

## What reads the manifest

`validate_recipe_manifest` in
`crates/amplihack-cli/src/commands/install/bundle_compat.rs`, called by the
installed-bundle compatibility check, is the only production reader. It
requires that:

1. the file exists at `<bundle>/recipes/_recipe_manifest.json` and can be
   read;
2. its contents parse as JSON and the top level is an object;
3. there is an entry for `smart-orchestrator` and for each of
   `smart-classify-route`, `smart-execute-routing`, `smart-reflect-loop` and
   `smart-validate-summarize`;
4. each of those five values is a string that is not empty after trimming.

That is the whole contract. The validator never hashes a recipe and never
compares an entry with a recipe's contents, and every entry outside those
five is never looked at. A manifest whose every digest is wrong passes, as does one whose
required entries read `"x"`.

So the manifest answers "did this recipe's text change between two trees that
both keep the file current", and nothing more. It is not bundle integrity
verification, it does not detect tampering, and no run refuses a recipe
because of it. Treat a stale entry as untidy, not as a vulnerability. The
controls that do protect a run's recipes and tools are the bundle trust
checks and the per-run snapshot described in
[Recipe run bundle selection and pinning](./recipe-run-bundle.md).

## What keeps entries current

`manifest_hashes_match_edited_recipes` in
`tests/integration/issue_1966_consensus_criticality.rs` compares the manifest
entry with `sha256sum` of the recipe for every name in that file's
`EDITED_RECIPES` list, and fails with
`_recipe_manifest.json entry for <name> is stale`.

The list holds the recipes whose text recent changes own, including two named
only so that a merge from main cannot leave their entries stale unnoticed. A
recipe outside the list can carry a stale entry without failing any test,
which is why the regeneration command exists and why regenerating is part of
editing a recipe. When a change makes a recipe's text its own, add that
recipe's name to `EDITED_RECIPES`.

## Install-time errors

Each message is prefixed `framework bundle compatibility check failed:` and
comes from `BundleCompatibilityError`:

| When | Message |
|------|---------|
| File absent | `missing recipe manifest at <path>` |
| File unreadable | `failed to read recipe manifest <path>: <system error>` |
| Not valid JSON | `invalid recipe manifest JSON in <path>: <parse error>` |
| Top level is not an object | `recipe manifest <path> is not a JSON object` |
| A required entry is absent | `recipe manifest <path> is missing required entry <recipe>` |
| A required entry is not a non-empty string | `recipe manifest <path> has an invalid hash entry for <recipe>` |

A wrong digest for a required entry is not in the list, because it is not
checked. See
[Framework bundle compatibility](./framework-bundle-compatibility.md) for the
rest of the compatibility contract and
[Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md)
for fixing an install that fails it.

## Related

- [Framework bundle compatibility](./framework-bundle-compatibility.md): the compatibility contract that reads the manifest
- [Recipe run bundle selection and pinning](./recipe-run-bundle.md): the checks that do protect a run's recipes and tools
- [Recipe runner overview](../recipes/README.md): recipe YAML format and the bundled recipe set
- [Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md): what to do when an install is refused
