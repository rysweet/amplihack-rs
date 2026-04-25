"""Language-blind graph data contract used by all analyzers and the renderer.

Brick philosophy: this module defines plain dataclasses (a *data contract*),
not a base class. Analyzers may import these dataclasses but MUST NOT inherit
from one another.
"""

from __future__ import annotations

import os
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class Node:
    id: str
    label: str
    language: str
    file_path: str


@dataclass
class Edge:
    src: str
    dst: str
    kind: str


@dataclass
class Graph:
    language: str
    nodes: list[Node] = field(default_factory=list)
    edges: list[Edge] = field(default_factory=list)


def common_root(paths: Iterable[Path]) -> Path | None:
    """Return the longest common parent directory of `paths`, or None for empty input.

    Used by analyzers to build collision-free, repo-relative node ids
    (e.g. so two `mod.rs` files in different crates do not collapse into
    one graph node — see #363 / COE feedback).
    """
    materialised = [p.resolve() for p in paths if p is not None]
    if not materialised:
        return None
    if len(materialised) == 1:
        # Use the file's parent so the relative path keeps the basename.
        return materialised[0].parent
    common = os.path.commonpath([str(p) for p in materialised])
    return Path(common)


def make_node_id(path: Path, root: Path | None) -> str:
    """Build a unique node id for `path` relative to `root`.

    Falls back to the absolute path (with separators normalised) when no
    root is available. The id is always unique for a given input path
    (no two distinct files collide), in contrast to `path.stem`.
    """
    p = Path(path).resolve()
    if root is not None:
        try:
            rel = p.relative_to(root)
        except ValueError:
            rel = p
    else:
        rel = p
    # Mermaid-safe: replace path separators with `/` (already canonical)
    # and drop the suffix from the trailing component to keep ids readable
    # while still unique (the directory prefix carries the disambiguation).
    parts = list(rel.parts)
    if parts:
        parts[-1] = Path(parts[-1]).stem
    return "/".join(parts) if parts else p.stem
