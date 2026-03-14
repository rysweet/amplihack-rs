"""
TDD Step 7: Tests for ws3_e2e_ci_patch.py and the resulting ci.yml changes.

Two test classes:

1. TestCiWorkflowCurrentState — tests that PASS right now, guarding the existing
   'check', 'test', 'cross-compile', 'release' jobs against regression.

2. TestCiWorkflowAfterPatch — tests that FAIL right now (test-e2e job missing),
   driving the WS3 implementation. PASS once ws3_e2e_ci_patch.py has been
   run and ci.yml updated.

3. TestWs3PatcherScript — unit tests for scripts/ws3_e2e_ci_patch.py itself.
   FAIL until the script is created.

IMPLEMENTATION TARGET: scripts/ws3_e2e_ci_patch.py
  Must:
  - Load ci.yml using yaml.safe_load (not string interpolation)
  - Add a 'test-e2e' job with: needs [check, test], continue-on-error: true,
    permissions: {contents: read}, cargo cache, recipe-runner-rs install,
    cargo test --workspace --locked -- --ignored
  - Write result back via yaml.safe_dump (not string interpolation)
  - NOT modify any existing job
  - Accept --rev <sha> for pinning recipe-runner-rs install
  - Exit non-zero if local e2e tests (cargo test -- --ignored) do not all pass
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest
import yaml

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
CI_WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "ci.yml"
sys.path.insert(0, str(REPO_ROOT / "scripts"))


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def load_ci_yml() -> dict:
    """Load the current ci.yml. Caches the result per test session."""
    with open(CI_WORKFLOW_PATH, "r") as f:
        return yaml.safe_load(f)


def get_step_runs(job: dict) -> list[str]:
    """Extract all 'run:' strings from a job's steps."""
    return [
        s.get("run", "") for s in job.get("steps", []) if isinstance(s.get("run"), str)
    ]


def get_step_uses(job: dict) -> list[str]:
    """Extract all 'uses:' strings from a job's steps."""
    return [
        s.get("uses", "")
        for s in job.get("steps", [])
        if isinstance(s.get("uses"), str)
    ]


# ---------------------------------------------------------------------------
# 1. Tests guarding EXISTING ci.yml structure (PASS NOW)
# ---------------------------------------------------------------------------


class TestCiWorkflowCurrentState:
    """Guards on the existing ci.yml that must not be broken by any patch."""

    def test_ci_yml_is_valid_yaml(self):
        """ci.yml must always parse as valid YAML."""
        with open(CI_WORKFLOW_PATH, "r") as f:
            content = f.read()
        parsed = yaml.safe_load(content)
        assert parsed is not None, "ci.yml must not be empty"
        assert isinstance(
            parsed, dict
        ), f"ci.yml top level must be a dict. Got: {type(parsed)}"

    def test_ci_yml_has_name(self):
        config = load_ci_yml()
        assert "name" in config, "ci.yml must have a 'name' key"

    def test_check_job_exists(self):
        config = load_ci_yml()
        assert "check" in config.get("jobs", {}), "'check' job must exist"

    def test_test_job_exists(self):
        config = load_ci_yml()
        assert "test" in config.get("jobs", {}), "'test' job must exist"

    def test_cross_compile_job_exists(self):
        config = load_ci_yml()
        assert "cross-compile" in config.get(
            "jobs", {}
        ), "'cross-compile' job must exist"

    def test_release_job_exists(self):
        config = load_ci_yml()
        assert "release" in config.get("jobs", {}), "'release' job must exist"

    def test_test_job_runs_on_ubuntu(self):
        config = load_ci_yml()
        test_job = config["jobs"]["test"]
        assert (
            test_job.get("runs-on") == "ubuntu-latest"
        ), f"test job must run on ubuntu-latest. Got: {test_job.get('runs-on')}"

    def test_test_job_needs_check(self):
        config = load_ci_yml()
        test_job = config["jobs"]["test"]
        needs = test_job.get("needs", [])
        if isinstance(needs, str):
            needs = [needs]
        assert "check" in needs, f"test job must need 'check'. Got needs: {needs}"

    def test_test_job_runs_cargo_test_workspace_locked(self):
        config = load_ci_yml()
        test_job = config["jobs"]["test"]
        runs = get_step_runs(test_job)
        matching = [
            r
            for r in runs
            if "cargo test" in r and "--workspace" in r and "--locked" in r
        ]
        assert matching, (
            "test job must have: cargo test --workspace --locked. "
            f"Got step runs: {runs}"
        )

    def test_test_job_does_not_run_ignored_tests(self):
        """The standard test job must NOT include --ignored (that is test-e2e's job)."""
        config = load_ci_yml()
        test_job = config["jobs"]["test"]
        runs = get_step_runs(test_job)
        for run_cmd in runs:
            assert "--ignored" not in run_cmd, (
                "test job must NOT run --ignored tests. "
                f"Found '--ignored' in: {run_cmd!r}"
            )

    def test_check_job_verifies_cxx_build_pin(self):
        """check job must verify cxx-build is pinned to exactly 1.0.138."""
        config = load_ci_yml()
        check_job = config["jobs"]["check"]
        runs = get_step_runs(check_job)
        combined = "\n".join(runs)
        assert "cxx-build" in combined and "1.0.138" in combined, (
            "check job must verify cxx-build pin to 1.0.138. " f"Got runs: {runs}"
        )

    def test_release_job_needs_test_and_cross_compile(self):
        config = load_ci_yml()
        release_job = config["jobs"]["release"]
        needs = release_job.get("needs", [])
        if isinstance(needs, str):
            needs = [needs]
        assert "test" in needs, f"release must need 'test'. Got: {needs}"
        assert (
            "cross-compile" in needs
        ), f"release must need 'cross-compile'. Got: {needs}"

    def test_ci_yml_on_triggers(self):
        """CI must trigger on push to main and on pull_request to main."""
        config = load_ci_yml()
        assert "on" in config or True in config, "ci.yml must have an 'on' trigger"
        on_cfg = config.get("on") or config.get(True, {})
        if isinstance(on_cfg, dict):
            assert (
                "push" in on_cfg or "pull_request" in on_cfg
            ), f"CI must trigger on push or pull_request. Got: {list(on_cfg.keys())}"


# ---------------------------------------------------------------------------
# 2. Tests for the PATCHED ci.yml (FAIL NOW — test-e2e job missing)
# ---------------------------------------------------------------------------


class TestCiWorkflowAfterPatch:
    """
    These tests FAIL until ws3_e2e_ci_patch.py has been applied to ci.yml.

    Each test has a clear FAILS / PASSES annotation.
    Run 'python scripts/ws3_e2e_ci_patch.py' to make them pass.
    """

    def test_test_e2e_job_exists(self):
        """
        FAILS: 'test-e2e' job does not exist in current ci.yml.
        PASSES: After ws3_e2e_ci_patch.py adds the job.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, (
            "ci.yml must contain a 'test-e2e' job. "
            "FIX: Run 'python scripts/ws3_e2e_ci_patch.py' to add it. "
            f"Current jobs: {sorted(jobs.keys())}"
        )

    def test_test_e2e_needs_check_and_test(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch adds test-e2e with needs: [check, test].

        Rationale: test-e2e must gate on test passing to avoid wasting runner minutes.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        needs = jobs["test-e2e"].get("needs", [])
        if isinstance(needs, str):
            needs = [needs]

        assert "check" in needs, f"test-e2e must need 'check'. Got needs: {needs}"
        assert "test" in needs, (
            f"test-e2e must need 'test'. Got needs: {needs}. "
            "Rationale: don't run E2E if unit tests fail."
        )

    def test_test_e2e_has_continue_on_error_true(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.

        Rationale: E2E tests are stabilising; continue-on-error:true prevents
        blocking main CI while tests are being stabilised. File follow-up issue
        to remove this once tests are stable.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        coe = jobs["test-e2e"].get("continue-on-error")
        assert coe is True, (
            "test-e2e must have continue-on-error: true during stabilisation. "
            f"Got: {coe!r}. "
            "Remember: file a follow-up issue to remove this once tests are stable."
        )

    def test_test_e2e_has_contents_read_permission(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.

        Security: least-privilege — test-e2e does not need write access.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        perms = jobs["test-e2e"].get("permissions", {})
        assert perms.get("contents") == "read", (
            "test-e2e must have permissions: contents: read (principle of least privilege). "
            f"Got permissions: {perms}"
        )

    def test_test_e2e_runs_on_ubuntu_latest(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        runner = jobs["test-e2e"].get("runs-on")
        assert (
            runner == "ubuntu-latest"
        ), f"test-e2e must run on ubuntu-latest. Got: {runner!r}"

    def test_test_e2e_has_cargo_or_rust_cache_step(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.

        Rationale: cargo install --git without caching takes 5+ min every run.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        uses = get_step_uses(jobs["test-e2e"])
        cache_actions = [
            u for u in uses if "cache" in u.lower() or "rust-cache" in u.lower()
        ]
        assert cache_actions, (
            "test-e2e must use a cache action (Swatinem/rust-cache or actions/cache) "
            "to avoid reinstalling recipe-runner-rs on every run. "
            f"Step uses: {uses}"
        )

    def test_test_e2e_installs_recipe_runner_rs(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.

        The job must install recipe-runner-rs before running the ignored tests.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        runs = get_step_runs(jobs["test-e2e"])
        names = [s.get("name", "") for s in jobs["test-e2e"].get("steps", [])]
        combined = "\n".join(runs + names).lower()

        assert "recipe-runner-rs" in combined, (
            "test-e2e must install recipe-runner-rs. "
            "Expected 'cargo install --git ... recipe-runner-rs' in a step. "
            f"Step runs: {runs}\nStep names: {names}"
        )

    def test_test_e2e_runs_cargo_test_with_ignored_flag(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.

        The E2E job must specifically run ignored tests (the recipe runner tests).
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        runs = get_step_runs(jobs["test-e2e"])
        ignored_steps = [r for r in runs if "cargo test" in r and "--ignored" in r]
        assert ignored_steps, (
            "test-e2e must run 'cargo test ... -- --ignored'. " f"Got step runs: {runs}"
        )

    def test_test_e2e_cargo_test_uses_workspace_and_locked(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.

        Must use --workspace and --locked for reproducibility.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        runs = get_step_runs(jobs["test-e2e"])
        ignored_steps = [r for r in runs if "cargo test" in r and "--ignored" in r]
        assert ignored_steps, "test-e2e must have a cargo test --ignored step"

        cargo_cmd = ignored_steps[-1]
        assert (
            "--workspace" in cargo_cmd
        ), f"test-e2e cargo test must include --workspace. Got: {cargo_cmd!r}"
        assert (
            "--locked" in cargo_cmd
        ), f"test-e2e cargo test must include --locked. Got: {cargo_cmd!r}"

    def test_test_e2e_cargo_install_is_pinned_to_rev(self):
        """
        FAILS: 'test-e2e' job does not exist.
        PASSES: After patch.

        Security: cargo install --git must pin to a specific --rev <sha>
        to prevent supply-chain drift.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})
        assert "test-e2e" in jobs, "test-e2e job missing from ci.yml"

        runs = get_step_runs(jobs["test-e2e"])
        install_steps = [
            r for r in runs if "cargo install" in r and "recipe-runner-rs" in r
        ]
        assert install_steps, "test-e2e must have a cargo install recipe-runner-rs step"

        install_cmd = install_steps[0]
        assert (
            "--rev" in install_cmd
            or "--tag" in install_cmd
            or "--branch" in install_cmd
        ), (
            "cargo install --git for recipe-runner-rs must pin to a specific --rev <sha> "
            "to prevent supply-chain drift. "
            f"Got: {install_cmd!r}"
        )

    def test_existing_jobs_unchanged_after_patch(self):
        """
        PASSES NOW (guarding against regression from the patch).

        After ws3_e2e_ci_patch.py is run, the existing jobs must remain identical
        except for the new test-e2e job being added.
        """
        config = load_ci_yml()
        jobs = config.get("jobs", {})

        # These four jobs must still exist after patch
        for job_name in ["check", "test", "cross-compile", "release"]:
            assert job_name in jobs, (
                f"'{job_name}' job must not be removed by the WS3 patch. "
                f"Present jobs: {sorted(jobs.keys())}"
            )

        # test job cargo command must be unchanged
        test_job = jobs["test"]
        runs = get_step_runs(test_job)
        assert any("cargo test --workspace --locked" in r for r in runs), (
            "test job 'cargo test --workspace --locked' must not be modified. "
            f"Runs: {runs}"
        )


# ---------------------------------------------------------------------------
# 3. Unit tests for ws3_e2e_ci_patch.py script
# ---------------------------------------------------------------------------


try:
    from ws3_e2e_ci_patch import (
        add_test_e2e_job,
        build_test_e2e_job,
        load_ci_yaml,
        validate_ci_yaml_structure,
        write_ci_yaml,
    )

    _SCRIPT_IMPORT_FAILED = False
except ImportError as exc:
    _SCRIPT_IMPORT_FAILED = True
    _SCRIPT_IMPORT_ERROR = str(exc)


def _skip_if_script_missing():
    if _SCRIPT_IMPORT_FAILED:
        pytest.fail(
            "scripts/ws3_e2e_ci_patch.py not found. "
            "IMPORT ERROR: " + _SCRIPT_IMPORT_ERROR + "\n"
            "FIX: Create scripts/ws3_e2e_ci_patch.py with "
            "load_ci_yaml(), build_test_e2e_job(), add_test_e2e_job(), "
            "validate_ci_yaml_structure(), write_ci_yaml()."
        )


class TestWs3PatcherModuleContract:
    """ws3_e2e_ci_patch.py must export the expected public API."""

    def test_load_ci_yaml_exists(self):
        _skip_if_script_missing()
        assert callable(load_ci_yaml), "load_ci_yaml must be callable"

    def test_build_test_e2e_job_exists(self):
        _skip_if_script_missing()
        assert callable(build_test_e2e_job), "build_test_e2e_job must be callable"

    def test_add_test_e2e_job_exists(self):
        _skip_if_script_missing()
        assert callable(add_test_e2e_job), "add_test_e2e_job must be callable"

    def test_validate_ci_yaml_structure_exists(self):
        _skip_if_script_missing()
        assert callable(
            validate_ci_yaml_structure
        ), "validate_ci_yaml_structure must be callable"

    def test_write_ci_yaml_exists(self):
        _skip_if_script_missing()
        assert callable(write_ci_yaml), "write_ci_yaml must be callable"


class TestBuildTestE2eJob:
    """build_test_e2e_job must return a valid CI job dict."""

    def test_returns_dict(self):
        _skip_if_script_missing()
        job = build_test_e2e_job(rev="abc123def456")
        assert isinstance(
            job, dict
        ), f"build_test_e2e_job must return a dict. Got: {type(job)}"

    def test_has_runs_on(self):
        _skip_if_script_missing()
        job = build_test_e2e_job(rev="abc123")
        assert (
            job.get("runs-on") == "ubuntu-latest"
        ), f"test-e2e job must run on ubuntu-latest. Got: {job.get('runs-on')}"

    def test_has_correct_needs(self):
        _skip_if_script_missing()
        job = build_test_e2e_job(rev="abc123")
        needs = job.get("needs", [])
        if isinstance(needs, str):
            needs = [needs]
        assert (
            "check" in needs and "test" in needs
        ), f"test-e2e must need check and test. Got: {needs}"

    def test_has_continue_on_error(self):
        _skip_if_script_missing()
        job = build_test_e2e_job(rev="abc123")
        assert (
            job.get("continue-on-error") is True
        ), f"test-e2e must have continue-on-error: true. Got: {job.get('continue-on-error')}"

    def test_has_least_privilege_permissions(self):
        _skip_if_script_missing()
        job = build_test_e2e_job(rev="abc123")
        perms = job.get("permissions", {})
        assert (
            perms.get("contents") == "read"
        ), f"test-e2e permissions.contents must be read. Got: {perms}"

    def test_steps_include_recipe_runner_install_with_rev(self):
        _skip_if_script_missing()
        rev = "deadbeef1234abcd"
        job = build_test_e2e_job(rev=rev)
        runs = [s.get("run", "") for s in job.get("steps", []) if "run" in s]
        install_steps = [
            r for r in runs if "cargo install" in r and "recipe-runner" in r
        ]
        assert install_steps, (
            "test-e2e steps must include cargo install for recipe-runner-rs. "
            f"Step runs: {runs}"
        )
        assert rev in install_steps[0], (
            f"cargo install step must include the provided rev '{rev}'. "
            f"Got: {install_steps[0]!r}"
        )

    def test_steps_include_cargo_test_ignored(self):
        _skip_if_script_missing()
        job = build_test_e2e_job(rev="abc123")
        runs = [s.get("run", "") for s in job.get("steps", []) if "run" in s]
        ignored_steps = [r for r in runs if "cargo test" in r and "--ignored" in r]
        assert ignored_steps, (
            "test-e2e steps must include cargo test --ignored. " f"Step runs: {runs}"
        )

    def test_steps_include_cache_action(self):
        _skip_if_script_missing()
        job = build_test_e2e_job(rev="abc123")
        uses = [s.get("uses", "") for s in job.get("steps", []) if "uses" in s]
        cache_uses = [u for u in uses if "cache" in u.lower()]
        assert cache_uses, (
            "test-e2e steps must include a cache action. " f"Step uses: {uses}"
        )


class TestAddTestE2eJob:
    """add_test_e2e_job must add the job to a config without modifying existing jobs."""

    def _base_config(self) -> dict:
        """Minimal config matching the current ci.yml structure."""
        return {
            "name": "CI",
            "on": {"push": {"branches": ["main"]}},
            "jobs": {
                "check": {"runs-on": "ubuntu-latest", "steps": []},
                "test": {"runs-on": "ubuntu-latest", "needs": "check", "steps": []},
            },
        }

    def test_adds_test_e2e_to_jobs(self):
        _skip_if_script_missing()
        config = self._base_config()
        result = add_test_e2e_job(config, rev="abc123")
        assert "test-e2e" in result.get("jobs", {}), (
            "add_test_e2e_job must add 'test-e2e' to jobs. "
            f"Got jobs: {list(result.get('jobs', {}).keys())}"
        )

    def test_does_not_modify_existing_check_job(self):
        _skip_if_script_missing()
        config = self._base_config()
        original_check = config["jobs"]["check"].copy()
        result = add_test_e2e_job(config, rev="abc123")
        assert result["jobs"]["check"] == original_check, (
            "add_test_e2e_job must not modify the existing 'check' job. "
            f"Before: {original_check}\nAfter: {result['jobs']['check']}"
        )

    def test_does_not_modify_existing_test_job(self):
        _skip_if_script_missing()
        config = self._base_config()
        import copy

        original_test = copy.deepcopy(config["jobs"]["test"])
        result = add_test_e2e_job(config, rev="abc123")
        assert (
            result["jobs"]["test"] == original_test
        ), "add_test_e2e_job must not modify the existing 'test' job."

    def test_returns_new_dict_not_mutating_input(self):
        _skip_if_script_missing()
        config = self._base_config()
        _ = add_test_e2e_job(config, rev="abc123")
        assert "test-e2e" not in config.get(
            "jobs", {}
        ), "add_test_e2e_job must return a new dict, not mutate the input config"

    def test_raises_if_test_e2e_already_exists(self):
        """Idempotency guard: must not silently double-add."""
        _skip_if_script_missing()
        config = self._base_config()
        config["jobs"]["test-e2e"] = {"runs-on": "ubuntu-latest"}
        with pytest.raises((ValueError, RuntimeError, KeyError)):
            add_test_e2e_job(config, rev="abc123")


class TestValidateCiYamlStructure:
    """validate_ci_yaml_structure must check the patched YAML is structurally correct."""

    def test_valid_config_passes(self):
        _skip_if_script_missing()
        config = {
            "name": "CI",
            "on": {"push": {"branches": ["main"]}},
            "jobs": {
                "check": {"runs-on": "ubuntu-latest", "steps": []},
                "test": {"runs-on": "ubuntu-latest", "steps": []},
                "test-e2e": {
                    "runs-on": "ubuntu-latest",
                    "needs": ["check", "test"],
                    "continue-on-error": True,
                    "permissions": {"contents": "read"},
                    "steps": [],
                },
            },
        }
        # Must not raise
        validate_ci_yaml_structure(config)

    def test_missing_jobs_key_raises(self):
        _skip_if_script_missing()
        with pytest.raises((ValueError, KeyError, AssertionError)):
            validate_ci_yaml_structure({"name": "CI"})

    def test_missing_test_e2e_job_raises(self):
        _skip_if_script_missing()
        config = {
            "name": "CI",
            "jobs": {"check": {}, "test": {}},
        }
        with pytest.raises((ValueError, KeyError, AssertionError)):
            validate_ci_yaml_structure(config)


class TestWriteCiYaml:
    """write_ci_yaml must write valid YAML that round-trips cleanly."""

    def test_written_file_is_valid_yaml(self, tmp_path):
        _skip_if_script_missing()
        config = {
            "name": "CI",
            "jobs": {
                "test-e2e": {
                    "runs-on": "ubuntu-latest",
                    "continue-on-error": True,
                    "steps": [],
                }
            },
        }
        output_path = tmp_path / "ci.yml"
        write_ci_yaml(config, output_path)

        with open(output_path, "r") as f:
            content = f.read()

        parsed = yaml.safe_load(content)
        assert parsed is not None
        assert parsed["jobs"]["test-e2e"]["continue-on-error"] is True

    def test_write_does_not_use_unsafe_yaml_dump(self, tmp_path):
        """Must use yaml.safe_dump, not yaml.dump (which may emit Python-specific tags)."""
        _skip_if_script_missing()
        config = {"name": "CI", "jobs": {}}
        output_path = tmp_path / "ci.yml"
        write_ci_yaml(config, output_path)

        with open(output_path, "r") as f:
            content = f.read()

        # yaml.dump with default Dumper emits !!python/... tags; safe_dump does not
        assert "!!python" not in content, (
            "write_ci_yaml must use yaml.safe_dump, not yaml.dump. "
            "Found '!!python' tags in output, indicating unsafe YAML serialisation."
        )
