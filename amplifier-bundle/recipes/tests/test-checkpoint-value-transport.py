"""Exercise the fixed checkpoint data reader with the selected real Bash."""
from pathlib import Path
import os
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
FIELDS = (
    "WF_CHECKPOINT_RECORD", "WF_CHECKPOINT_RECORD_ID", "WF_CHECKPOINT_HASH",
    "WF_CHECKPOINT_INVOCATION", "WF_CHECKPOINT_HEAD", "WF_CHECKPOINT_UPSTREAM",
    "WF_CHECKPOINT_UPSTREAM_REF", "WF_CHECKPOINT_BASE_REF",
)


class CheckpointValueTransport(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="checkpoint-values-", dir=tempfile.gettempdir())
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        source = (ROOT / "amplifier-bundle/tools/workflow_publish_checkpoint.sh").read_text()
        start = source.index("workflow_checkpoint_read_values() {\n")
        end = source.index("\n}\n\n# The wrapper observes", start) + 3
        self.function = source[start:end]
        self.bash = os.environ.get("CHECKPOINT_TEST_BASH", "/bin/bash")
        self.initial = ["initial-" + field for field in FIELDS]

    def run_reader(self, mode, payload):
        data = self.root / "values.bin"
        data.write_bytes(payload)
        script = self.root / "reader.sh"
        initial = "\n".join("export " + field + "=" + shlex.quote(value)
                            for field, value in zip(FIELDS, self.initial))
        observed = " ".join('"$' + field + '"' for field in FIELDS)
        script.write_text("set -u\n" + self.function + "\n" + initial +
                          '\nworkflow_checkpoint_read_values "$1" "$2"\nstatus=$?\n' +
                          "printf '%s\\0' \"$status\" " + observed + "\n")
        result = subprocess.run([self.bash, str(script), mode, str(data)],
                                env={"HOME": str(self.root), "TMPDIR": str(self.root),
                                     "PATH": os.defpath, "LC_ALL": "C"},
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual(result.returncode, 0, result.stderr)
        parts = result.stdout.split(b"\0")
        self.assertEqual(parts[-1], b"")
        self.assertEqual(len(parts), len(FIELDS) + 2)
        return int(parts[0]), [value.decode(errors="surrogateescape") for value in parts[1:-1]]

    @staticmethod
    def encoded(values):
        return b"\0".join(value.encode() for value in values) + b"\0"

    def assert_refused_unchanged(self, mode, payload):
        status, state = self.run_reader(mode, payload)
        self.assertEqual(status, 78)
        self.assertEqual(state, self.initial)

    def test_admit_preserves_hostile_values_without_evaluating_them(self):
        marker = self.root / "must-not-exist"
        value = 'quotes\'"; $(touch ' + str(marker) + '); `touch ' + str(marker) + '`\nline\\tail'
        values = [value, "", "hash", "invocation", "head", "upstream", "ref", "base"]
        status, state = self.run_reader("admit", self.encoded(values))
        self.assertEqual(status, 0)
        self.assertEqual(state, values)
        self.assertFalse(marker.exists())

    def test_after_preserves_immutable_fields_and_literal_state(self):
        values = ["head\nnext", "", "refs/heads/literal'branch", "base\\tail"]
        status, state = self.run_reader("after", self.encoded(values))
        self.assertEqual(status, 0)
        self.assertEqual(state[:4], self.initial[:4])
        self.assertEqual(state[4:], values)

    def test_non_utf8_bytes_remain_data(self):
        value = b"literal-\xff"
        status, state = self.run_reader("admit", b"\0".join([value] * 8) + b"\0")
        self.assertEqual(status, 0)
        self.assertEqual(state, [os.fsdecode(value)] * 8)

    def test_empty_fields_are_data_in_both_modes(self):
        for mode, count in (("admit", 8), ("after", 4)):
            with self.subTest(mode=mode):
                status, state = self.run_reader(mode, self.encoded([""] * count))
                self.assertEqual(status, 0)
                self.assertEqual(state, [""] * 8 if mode == "admit" else self.initial[:4] + [""] * 4)

    def test_empty_stream_refuses_before_state_assignment(self):
        for mode in ("admit", "after"):
            with self.subTest(mode=mode):
                self.assert_refused_unchanged(mode, b"")

    def test_missing_field_refuses_before_state_assignment(self):
        for mode, count in (("admit", 8), ("after", 4)):
            with self.subTest(mode=mode):
                self.assert_refused_unchanged(mode, self.encoded(["changed"] * (count - 1)))

    def test_missing_final_delimiter_refuses_before_state_assignment(self):
        for mode, count in (("admit", 8), ("after", 4)):
            with self.subTest(mode=mode):
                self.assert_refused_unchanged(mode, self.encoded(["changed"] * count)[:-1])

    def test_extra_empty_field_refuses_before_state_assignment(self):
        for mode, count in (("admit", 8), ("after", 4)):
            with self.subTest(mode=mode):
                self.assert_refused_unchanged(mode, self.encoded(["changed"] * count + [""]))

    def test_trailing_bytes_refuse_before_state_assignment(self):
        for mode, count in (("admit", 8), ("after", 4)):
            with self.subTest(mode=mode):
                self.assert_refused_unchanged(mode, self.encoded(["changed"] * count) + b"trailing")

    def test_unknown_mode_refuses_before_state_assignment(self):
        self.assert_refused_unchanged("unknown", self.encoded(["changed"] * 8))


if __name__ == "__main__":
    unittest.main(verbosity=2)
