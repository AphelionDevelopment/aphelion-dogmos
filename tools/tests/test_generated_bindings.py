import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "crates" / "dogmos-byond"
BINDINGS = CRATE / "bindings.dm"


class GeneratedBindingsTests(unittest.TestCase):
    def test_generation_is_stable_and_emits_canonical_bytes(self):
        committed = BINDINGS.read_bytes()
        try:
            first = self._generate()
            second = self._generate()
        finally:
            BINDINGS.write_bytes(committed)

        self.assertEqual(first, second)
        self.assertEqual(committed, first)
        self.assertNotIn(b"\r", first)
        self.assertFalse(any(line.endswith((b" ", b"\t")) for line in first.splitlines()))
        proc_paths = [
            line.split(b"(", 1)[0]
            for line in first.splitlines()
            if line.startswith(b"/proc/dogmos_")
        ]
        self.assertEqual(proc_paths, sorted(proc_paths))
        self.assertTrue(first.endswith(b"\n"))
        self.assertFalse(first.endswith(b"\n\n"))

    def _generate(self):
        completed = subprocess.run(
            [
                "cargo",
                "run",
                "--quiet",
                "--locked",
                "--target",
                "i686-pc-windows-msvc",
                "-p",
                "dogmos-byond",
                "--example",
                "generate_bindings",
            ],
            cwd=CRATE,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        return BINDINGS.read_bytes()


if __name__ == "__main__":
    unittest.main()
