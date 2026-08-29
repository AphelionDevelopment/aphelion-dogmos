import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
MANIFEST_PATH = REPOSITORY_ROOT / "dogmos-build-manifest.toml"
MODULE_PATH = REPOSITORY_ROOT / "tools" / "DogmosBuildIdentity.psm1"


class BuildIdentityTests(unittest.TestCase):
    def test_capability_manifest_matches_protocol_and_default_features(self):
        build_manifest = tomllib.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
        cargo_manifest = tomllib.loads(
            (REPOSITORY_ROOT / "Cargo.toml").read_text(encoding="utf-8")
        )
        protocol_source = (
            REPOSITORY_ROOT / "crates" / "dogmos-protocol" / "src" / "lib.rs"
        ).read_text(encoding="utf-8")

        self.assertEqual(build_manifest["schema_version"], 1)
        self.assertEqual(build_manifest["toolchain"]["byond"], "516.1687")
        self.assertEqual(
            build_manifest["capabilities"]["features"],
            sorted(cargo_manifest["features"]["default"]),
        )
        self.assertEqual(
            build_manifest["protocol"]["abi_version"],
            int(re.search(r"DOGMOS_ABI_VERSION: u16 = (\d+)", protocol_source).group(1)),
        )
        self.assertEqual(
            build_manifest["protocol"]["protocol_version"],
            int(
                re.search(
                    r"DOGMOS_PROTOCOL_VERSION: u16 = (\d+)", protocol_source
                ).group(1)
            ),
        )

    def test_powershell_identity_uses_head_and_manifest_digest(self):
        command = (
            f"Import-Module '{MODULE_PATH}'; "
            f"Get-DogmosBuildIdentity -RepositoryRoot '{REPOSITORY_ROOT}' "
            "-AllowDirty | ConvertTo-Json -Compress"
        )
        result = subprocess.run(
            ["powershell", "-NoProfile", "-Command", command],
            check=True,
            capture_output=True,
            text=True,
        )
        identity = json.loads(result.stdout)
        head = subprocess.run(
            ["git", "rev-parse", "--verify", "HEAD"],
            cwd=REPOSITORY_ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        status = subprocess.run(
            ["git", "status", "--porcelain"],
            cwd=REPOSITORY_ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout

        self.assertEqual(identity["source_revision"], head)
        self.assertIsNotNone(identity["feature_fingerprint"], result.stdout)
        self.assertEqual(
            identity["feature_fingerprint"],
            hashlib.sha256(MANIFEST_PATH.read_bytes()).hexdigest(),
        )
        self.assertEqual(identity["dirty"], bool(status.strip()))

    def test_ipc_entrypoint_tools_export_the_compiled_identity(self):
        for relative_path in (
            "tools/benchmark_ipc.ps1",
            "tools/test_cross_bitness_ipc.ps1",
            "tools/test_process_isolation.ps1",
        ):
            with self.subTest(path=relative_path):
                text = (REPOSITORY_ROOT / relative_path).read_text(encoding="utf-8")
                self.assertIn("DogmosBuildIdentity.psm1", text)
                self.assertIn("DOGMOS_SOURCE_REVISION", text)
                self.assertIn("DOGMOS_FEATURE_FINGERPRINT", text)


if __name__ == "__main__":
    unittest.main()
