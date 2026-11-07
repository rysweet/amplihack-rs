"""Inspect commit objects and transitions; never stage or grant publication authority."""

import base64
import os


def commit_objects(git):
    return {line.split()[0].decode() for line in git("cat-file", "--batch-all-objects", "--batch-check=%(objectname) %(objecttype)").splitlines()
            if line.split()[1] == b"commit"}


def publication_history(git, before_objects, allowed):
    """Include unreachable new commits, every merge parent, deletions and mode changes.

    `allowed` explicitly specifies each permitted resulting entry (mode and bytes).
    An absent resulting entry is authorised only by an explicit None value.
    """
    commits = sorted(commit_objects(git) - before_objects)
    transitions = []

    def entry(commit, path):
        if commit is None:
            return None
        raw = git("ls-tree", "-z", commit, "--", path)
        if not raw:
            return None
        header, actual = raw.rstrip(b"\0").split(b"\t", 1)
        mode, kind, oid = header.split()
        if actual != os.fsencode(path):
            raise ValueError("Tree lookup returned a different path")
        return {"mode": mode.decode(), "type": kind.decode(), "oid": oid.decode(),
                "contents_base64": base64.b64encode(git("cat-file", kind.decode(), oid.decode())).decode()}

    for commit in commits:
        parents = git("rev-list", "--parents", "-n", "1", commit).decode().split()[1:]
        for parent in parents or [None]:
            paths = (git("diff", "--name-only", "--no-renames", "-z", parent, commit) if parent else
                     git("diff-tree", "--root", "--no-commit-id", "--name-only", "--no-renames", "-r", "-z", commit))
            for raw_path in paths.split(b"\0"):
                if not raw_path:
                    continue
                path = os.fsdecode(raw_path)
                old, new = entry(parent, path), entry(commit, path)
                expected = allowed.get(path)
                authorised = path in allowed and (new is None if expected is None else
                    new is not None and new["type"] == "blob" and all(new[k] == v for k, v in expected.items()))
                transitions.append({"commit": commit, "parent": parent, "path": path,
                                    "before": old, "after": new, "authorised": authorised})
    return {"created_commits": commits, "transitions": transitions,
            "publication_authorised": all(row["authorised"] for row in transitions)}


def destination_observation(git, reference, expected_tip, allowed):
    """Read the actual fixture destination independently of local commit creation."""
    tip = git("rev-parse", "--verify", reference).decode().strip()
    entries = {}
    for path in sorted(allowed):
        raw = git("ls-tree", "-z", tip, "--", path)
        if not raw:
            entries[path] = None
            continue
        header, actual = raw.rstrip(b"\0").split(b"\t", 1)
        mode, kind, oid = header.split()
        if actual != os.fsencode(path):
            raise ValueError("Destination lookup returned a different path")
        entries[path] = {"mode": mode.decode(), "type": kind.decode(),
                         "contents_base64": base64.b64encode(git("cat-file", kind.decode(), oid.decode())).decode()}
    correct = all(entries[path] is None if expected is None else
                  entries[path] is not None and entries[path]["type"] == "blob"
                  and all(entries[path][key] == value for key, value in expected.items())
                  for path, expected in allowed.items())
    return {"reference": reference, "tip": tip, "expected_tip": expected_tip,
            "entries": entries, "matches_local_head": tip == expected_tip,
            "contents_authorised": correct,
            "publication_observed": tip == expected_tip and correct}
