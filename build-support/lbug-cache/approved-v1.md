amplihack-rs deputy (spec owner for rysweet/amplihack-rs#1917): **go for v1**, cut down from the proposal in comment 6615844000. The aim is to relieve candide's load quickly; the larger design is v2.

**v1, approved:**
1. **Local cache only.** On the first build for a key (lbug version from Cargo.lock, target triple, C++ compiler and major version), build lbug once with its own cmake and copy the result to `~/.cache/amplihack/lbug/<key>/{lib,include}`, with a sha256 manifest. `lib/` must hold every static archive that lbug-0.15.4's `build.rs` links in static mode: `lbug` plus the third-party list in its `link_libraries()` `for lib in [...]` loop, the 18 archives you identified. `include/` holds the headers the FFI shim needs.
2. **Use it when it verifies.** Every amplihack-rs cargo build sets `LBUG_LIBRARY_DIR` and `LBUG_INCLUDE_DIR` to that path when the manifest verifies: the dev wrapper, the pre-commit and pre-push hook cargo, and CI's cache step.
3. **Fallback.** A missing entry or a manifest mismatch falls back to today's bundled build, with a one-line note.
4. **Out of scope for v1:** no vendoring, no `[patch]` of lbug, no change to release builds or `release.yml` / `publish-snapshot.yml`, no npm/uvx sources. Release builds never set the two variables, so there is no `-Wl,-rpath,<cache>` in any release artefact; add a check for that.
5. **Tests:** with a verified cache entry no cmake runs (no `build_bundled_cmake`, no `cc1plus` for lbug); a sha256 mismatch falls back to the bundled build; a release-profile build has no rpath into `~/.cache`.

**v2 (follow-up issue):** a shared release asset under a tag, cross-host, with signed provenance.

cc implements v1 on the Mac, against #1917; I review. Please keep the PR to v1 and note anything deferred in the PR body.

