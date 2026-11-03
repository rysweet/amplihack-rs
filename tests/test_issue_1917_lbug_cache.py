"""Local v1 production boundaries and a real, dependency-free Cargo fixture.

The fixture substitutes a tiny native producer for lbug's expensive CMake build.
It proves wrapper selection and Cargo fingerprint invalidation, not lbug ABI,
native compilation speedup, or database/linkage acceptance.
"""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import signal
import unittest

REPO = Path(__file__).resolve().parents[1]
ARCHIVES = ("lbug utf8proc antlr4_cypher antlr4_runtime re2 fastpfor parquet thrift "
            "snappy zstd miniz mbedtls brotlidec brotlicommon lz4 roaring_bitmap simsimd yyjson").split()
HEADERS = ("common/type_utils.h common/types/int128_t.h common/types/types.h "
           "common/types/value/nested.h common/types/value/node.h common/types/value/recursive_rel.h "
           "common/types/value/rel.h common/types/value/value.h main/lbug.h storage/storage_version_info.h").split()


class LocalCacheContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Query only the provisioned toolchain; never let rustup install one.
        result = subprocess.run(["rustup", "which", "rustc"], cwd=REPO, capture_output=True,
                                text=True, env={**os.environ, "RUSTUP_AUTO_INSTALL": "0"}, timeout=10)
        if result.returncode:
            raise RuntimeError("provision the pinned Rust toolchain before running Cargo contracts")
        cls.rustc = Path(result.stdout.strip()).resolve()
        cls.compiler = Path(shutil.which("c++")).resolve()

    def setUp(self):
        fixture = tempfile.TemporaryDirectory(prefix="issue-1917-cargo-")
        self.addCleanup(fixture.cleanup)
        self.root = Path(fixture.name).resolve()
        self.source = self.root / "source tree"
        self.source.mkdir()
        for name in ("scripts", "build-support/lbug-cache", "src", "home", "cargo-home", "tmp", "tools"):
            (self.source / name).mkdir(parents=True, mode=0o700)
        for name in ("cargo-with-lbug.sh",):
            shutil.copy2(REPO / "scripts" / name, self.source / "scripts" / name)
        for name in ("main.rs", "cache.rs"):
            shutil.copy2(REPO / "build-support/lbug-cache" / name,
                         self.source / "build-support/lbug-cache" / name)
        self.cache = self.root / "cache"
        self.trace = self.root / "trace"
        self.env = {
            "PATH": f"{self.rustc.parent}:/usr/bin:/bin",
            "HOME": str(self.source / "home"), "CARGO_HOME": str(self.source / "cargo-home"),
            "TMPDIR": str(self.source / "tmp"), "CARGO_BUILD_JOBS": "1",
            "CARGO_INCREMENTAL": "0", "RUSTUP_AUTO_INSTALL": "0",
            "AMPLIHACK_LBUG_CACHE_DIR": str(self.cache),
            "TEST_TRACE": str(self.trace), "TEST_COMPILER": str(self.compiler),
            "TEST_SOURCE": str(self.root / "lbug-0.15.4/lbug-src"),
        }
        manifest = '[package]\nname="lbug-fixture"\nversion="0.0.0"\nedition="2024"\n'
        (self.source / "Cargo.toml").write_text(manifest)
        (self.source / "src/main.rs").write_text('fn main() { println!("{}", env!("SELECTION")); }\n')
        source = Path(self.env["TEST_SOURCE"])
        for directory in ("src/include", "third_party/nlohmann_json", "third_party/fastpfor", "third_party/alp/include"):
            (source / directory).mkdir(parents=True)
        for header in HEADERS:
            path = source / "src/include" / header
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("#pragma once\n")
        build = r'''
use std::{env,fs,io::Write,path::PathBuf};
fn main() {
    println!("cargo:rerun-if-env-changed=CXXFLAGS");
    let managed = env::var_os("LBUG_LIBRARY_DIR").is_some();
    let mode = if managed { "managed" } else { "source" };
    writeln!(fs::OpenOptions::new().create(true).append(true).open(env::var("TEST_TRACE").unwrap()).unwrap(), "{mode}").unwrap();
    println!("cargo:rustc-env=SELECTION={mode}");
    if !managed {
        assert!(env::var_os("TEST_SOURCE_FAILURE").is_none(), "fixture source compilation failed");
        if let Ok(ready) = env::var("TEST_BUILD_READY") {
            fs::write(ready, "ready").unwrap();
            while !std::path::Path::new(&env::var("TEST_BUILD_RELEASE").unwrap()).exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let root = PathBuf::from(env::var("OUT_DIR").unwrap()).join("build");
        fs::create_dir_all(root.join("src/include")).unwrap();
        std::os::unix::fs::symlink("liblbug.0.15.4.dylib", root.join("src/liblbug.dylib")).ok();
        for name in "ARCHIVE_LIST".split_whitespace() {
            let bytes = if env::var_os("TEST_THIN_ARCHIVE").is_some() { b"!<thin>\n" } else { b"!<arch>\n" };
            fs::write(root.join("src").join(format!("lib{name}.a")), bytes).unwrap();
        }
        fs::write(root.join("CMakeCache.txt"), format!("CMAKE_CXX_COMPILER:FILEPATH={}\nCMAKE_HOME_DIRECTORY:INTERNAL={}\n", env::var("TEST_COMPILER").unwrap(), env::var("TEST_SOURCE").unwrap())).unwrap();
    }
}
'''.replace("ARCHIVE_LIST", " ".join(ARCHIVES))
        (self.source / "build.rs").write_text(build)

    def prepare_lock(self):
        # Cargo removes this unused registry entry in the tiny fixture. Restore
        # it for the production resolver; no registry or C++ lbug code is used.
        (self.source / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "lbug-fixture"\nversion = "0.0.0"\n'
            '[[package]]\nname = "lbug"\nversion = "0.15.4"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "7ff191ec195fb257dd7ca2adb76afd9a4b9a9f60fa27f1d87e5a958849fe9e6f"\n')

    def run_build(self, extra=None, args=None):
        self.prepare_lock()
        result = subprocess.run(["/bin/bash", str(self.source / "scripts/cargo-with-lbug.sh"),
                                 *(args or ["build", "--offline"])], cwd=self.source,
                                env={**self.env, **(extra or {})}, capture_output=True, timeout=45)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        return result

    def entries(self):
        return list(self.cache.glob("lbug-*/manifest.sha256"))

    def events(self):
        return self.trace.read_text().splitlines()

    def populate(self):
        result = self.run_build()
        self.assertIn(b"stored local entry", result.stderr)
        self.assertEqual(len(self.entries()), 1, result.stderr.decode())
        entry = self.entries()[0].parent
        for line in self.entries()[0].read_text().splitlines()[1:]:
            expected, size, path = line.split("  ", 2)
            data = (entry / path).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), expected)
            self.assertEqual(len(data), int(size))
        return self.entries()[0].parent

    def test_miss_hit_valid_warm_and_two_clean_targets(self):
        self.populate()
        self.assertEqual(self.events(), ["source"])
        self.run_build()
        self.assertEqual(self.events(), ["source", "managed"])
        self.run_build()
        self.assertEqual(self.events(), ["source", "managed"], "valid warm output should remain fresh")
        self.run_build({"CARGO_TARGET_DIR": str(self.root / "second target")})
        self.assertEqual(self.events(), ["source", "managed", "managed"])

    def test_corruption_invalidates_warm_managed_output_without_repair(self):
        entry = self.populate()
        self.run_build()
        manifest = (entry / "manifest.sha256").read_bytes()
        (entry / "lib/liblbug.a").write_bytes(b"!<arch>\ncorrupt")
        result = self.run_build()
        self.assertIn(b"rejected; using bundled source", result.stderr)
        self.assertEqual(self.events(), ["source", "managed", "source"])
        self.assertEqual((entry / "manifest.sha256").read_bytes(), manifest)

    def test_removed_or_unreadable_manifest_invalidates_warm_managed_output(self):
        for mutation in ("removed", "unreadable"):
            with self.subTest(mutation=mutation):
                # One entry can be restored from retained bytes between cases.
                if not self.entries():
                    self.populate()
                manifest = self.entries()[0]
                original = manifest.read_bytes()
                manifest.chmod(0o600)
                self.run_build()
                if mutation == "removed":
                    manifest.unlink()
                else:
                    manifest.chmod(0)
                before = len(self.events())
                self.run_build()
                self.assertEqual(self.events()[before:], ["source"])
                if manifest.exists():
                    manifest.chmod(0o600)
                manifest.write_bytes(original)

    def test_release_profile_never_receives_managed_paths(self):
        self.populate()
        result = self.run_build(args=["build", "--release", "--offline"])
        self.assertNotIn(b"verified local hit", result.stderr)
        self.assertEqual(self.events()[-1], "source")

    def test_unsafe_cache_directory_selects_source_without_writes(self):
        actual = self.root / "actual"
        actual.mkdir(mode=0o700)
        self.cache.symlink_to(actual, target_is_directory=True)
        self.run_build()
        self.assertEqual(self.events(), ["source"])
        self.assertEqual(list(actual.iterdir()), [])

    def test_extra_members_case_collisions_and_symlinked_payloads_reject(self):
        entry = self.populate()
        original_header = (entry / "include/lbug.hpp").read_bytes()
        for kind in ("extra", "case", "link"):
            with self.subTest(kind=kind):
                self.run_build()
                path = entry / ("unexpected" if kind == "extra" else "include/LBUG.hpp" if kind == "case" else "include/linked.h")
                if kind == "link":
                    path.symlink_to(entry / "include/lbug.hpp")
                else:
                    path.write_bytes(b"poison")
                result = self.run_build()
                self.assertIn(b"rejected; using bundled source", result.stderr)
                self.assertEqual(self.events()[-1], "source")
                path.unlink()
                (entry / "include/lbug.hpp").write_bytes(original_header)

    def test_packaging_failure_preserves_successful_source_without_second_callback(self):
        result = self.run_build({"TEST_THIN_ARCHIVE": "1"})
        self.assertIn(b"successful source output retained", result.stderr)
        self.assertEqual(self.events(), ["source"])
        self.assertEqual(self.entries(), [])
        self.assertEqual(list(self.cache.glob(".staging-*")), [])

    def test_native_settings_change_invalidates_warm_managed_output(self):
        self.populate()
        self.run_build()
        result = self.run_build({"CXXFLAGS": "-DREQUESTED_NATIVE_SETTING=1"})
        self.assertIn(b"compatibility rejected", result.stderr)
        self.assertEqual(self.events(), ["source", "managed", "source"])

    def test_cmake_tripwire_does_not_change_native_compatibility(self):
        self.populate()
        result = self.run_build({"CMAKE": str(self.root / "missing-cmake")})
        self.assertIn(b"verified local hit", result.stderr)
        self.assertEqual(self.events(), ["source", "managed"])

    def test_machine_specific_native_flags_select_source(self):
        self.populate()
        self.run_build()
        result = self.run_build({"CXXFLAGS": "-march=native"})
        self.assertNotIn(b"verified local hit", result.stderr)
        self.assertEqual(self.events(), ["source", "managed", "source"])

    def test_application_version_change_keeps_unchanged_native_cache(self):
        self.populate()
        manifest = self.source / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace('version="0.0.0"', 'version="0.0.1"'))
        result = self.run_build()
        self.assertIn(b"verified local hit", result.stderr)
        self.assertEqual(self.events(), ["source", "managed"])

    def test_source_compilation_failure_is_returned_without_packaging(self):
        self.prepare_lock()
        result = subprocess.run(["/bin/bash", str(self.source / "scripts/cargo-with-lbug.sh"), "build", "--offline"],
                                cwd=self.source, env={**self.env, "TEST_SOURCE_FAILURE": "1"},
                                capture_output=True, timeout=45)
        self.assertEqual(result.returncode, 101, result.stderr.decode())
        self.assertIn(b"fixture source compilation failed", result.stderr)
        self.assertNotIn(b"successful source output retained", result.stderr)
        self.assertEqual(self.events(), ["source"])
        self.assertEqual(self.entries(), [])

    def spawn_build(self, name, extra):
        log = open(self.root / (name + ".log"), "wb")
        self.addCleanup(log.close)
        child = subprocess.Popen(["/bin/bash", str(self.source / "scripts/cargo-with-lbug.sh"),
                                  "build", "--offline"], cwd=self.source,
                                 env={**self.env, **extra}, stdout=log, stderr=log)
        def cleanup():
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
        self.addCleanup(cleanup)
        return child

    def wait_for(self, predicate, description):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.01)
        self.fail(description)

    def test_competing_miss_waiter_revalidates_winner_before_build(self):
        ready, release = self.root / "ready", self.root / "release"
        self.prepare_lock()
        first = self.spawn_build("first", {"TEST_BUILD_READY": str(ready), "TEST_BUILD_RELEASE": str(release)})
        self.wait_for(ready.exists, "first source callback never reached synchronization point")
        lock = next(self.cache.glob("*.lock"))
        inode = lock.stat().st_ino
        self.prepare_lock()
        second = self.spawn_build("second", {"CARGO_TARGET_DIR": str(self.root / "second")})
        self.wait_for(lambda: b"waiting for local key lock" in (self.root / "second.log").read_bytes(),
                      "second producer never attempted production lock")
        self.assertEqual(self.events(), ["source"], "waiter must not compile before winner")
        release.touch()
        self.assertEqual(first.wait(timeout=30), 0, (self.root / "first.log").read_text())
        self.assertEqual(second.wait(timeout=30), 0, (self.root / "second.log").read_text())
        self.assertEqual(self.events(), ["source", "managed"])
        self.assertIn(b"verified local hit", (self.root / "second.log").read_bytes())
        self.assertEqual(lock.stat().st_ino, inode)

    def test_interrupted_source_releases_key_without_installing_partial_entry(self):
        ready, release = self.root / "ready", self.root / "release"
        self.prepare_lock()
        child = self.spawn_build("interrupted", {"TEST_BUILD_READY": str(ready), "TEST_BUILD_RELEASE": str(release)})
        self.wait_for(ready.exists, "source callback never reached synchronization point")
        child.send_signal(signal.SIGTERM)
        self.assertNotEqual(child.wait(timeout=10), 0)
        self.assertEqual(self.entries(), [])
        self.run_build()
        self.assertEqual(self.events(), ["source", "source"])
        self.assertEqual(len(self.entries()), 1)


if __name__ == "__main__":
    unittest.main()
