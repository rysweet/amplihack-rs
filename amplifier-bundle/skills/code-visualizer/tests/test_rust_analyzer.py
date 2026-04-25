"""Tests for rust_analyzer.normalize()."""

from __future__ import annotations

from pathlib import Path


def _write(p: Path, text: str) -> Path:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text, encoding="utf-8")
    return p


def test_use_statements(tmp_path: Path):
    from rust_analyzer import normalize

    f = _write(
        tmp_path / "src" / "main.rs",
        """
use std::collections::HashMap;
use crate::utils::helper;
use super::sibling;
mod local;
""",
    )
    g = normalize([f])
    assert g.language == "rust"
    dsts = {e.dst for e in g.edges}
    assert any("std" in d or "HashMap" in d or "collections" in d for d in dsts)
    assert any("utils" in d or "helper" in d for d in dsts)
    assert any("sibling" in d for d in dsts)
    assert any("local" in d for d in dsts)


def test_empty(tmp_path: Path):
    from rust_analyzer import normalize

    g = normalize([])
    assert g.language == "rust"
    assert g.edges == []


def test_no_node_id_collision_across_crates(tmp_path: Path):
    """#363: two `mod.rs` files in different crates must not share a node id."""
    from rust_analyzer import normalize

    a = _write(tmp_path / "crate_a" / "src" / "mod.rs", "use foo::bar;\n")
    b = _write(tmp_path / "crate_b" / "src" / "mod.rs", "use baz::qux;\n")

    g = normalize([a, b])
    ids = [n.id for n in g.nodes]
    assert len(ids) == 2, f"expected 2 distinct nodes, got {ids}"
    assert len(set(ids)) == 2, f"node ids collided: {ids}"
    # Labels may repeat (display name) — only ids must be unique.
    labels = {n.label for n in g.nodes}
    assert "mod" in labels
