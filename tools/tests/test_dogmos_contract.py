import hashlib
import json
from pathlib import Path
import shutil
import struct
import tempfile
import unittest

from tools.dogmos_contract import (
    ArtifactInput,
    ContractError,
    build_manifest,
    canonical_manifest_bytes,
    verify_manifest_bytes,
)


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]


def pe(machine: int) -> bytes:
    binary = bytearray(128)
    binary[:2] = b"MZ"
    struct.pack_into("<I", binary, 0x3C, 64)
    binary[64:68] = b"PE\0\0"
    struct.pack_into("<H", binary, 68, machine)
    return bytes(binary)


def elf(elf_class: int, machine: int) -> bytes:
    binary = bytearray(64)
    binary[:4] = b"\x7fELF"
    binary[4] = elf_class
    binary[5] = 1
    struct.pack_into("<H", binary, 18, machine)
    return bytes(binary)


class DogmosContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.bundle = Path(self.temporary.name)
        self.bindings = self.bundle / "dogmos_bindings.dm"
        self.bindings.write_bytes(b"#define DOGMOS_BYOND \"dogmos\"\n")
        fixtures = (
            ("windows", "shim", "i686-pc-windows-msvc", "i686", pe(0x014C)),
            ("windows", "service", "x86_64-pc-windows-msvc", "x86_64", pe(0x8664)),
            ("linux", "shim", "i686-unknown-linux-gnu", "i686", elf(1, 3)),
            ("linux", "service", "x86_64-unknown-linux-gnu", "x86_64", elf(2, 62)),
        )
        self.artifacts = []
        for platform, role, target, architecture, binary in fixtures:
            binary_name = f"{platform}/{role}.bin"
            symbols_name = f"{platform}/{role}.symbols"
            binary_path = self.bundle / binary_name
            symbols_path = self.bundle / symbols_name
            binary_path.parent.mkdir(parents=True, exist_ok=True)
            binary_path.write_bytes(binary)
            symbols_path.write_bytes(f"{target}-symbols".encode())
            self.artifacts.append(
                ArtifactInput(
                    platform=platform,
                    role=role,
                    target=target,
                    architecture=architecture,
                    binary_path=binary_path,
                    binary_name=binary_name,
                    symbols_path=symbols_path,
                    symbols_name=symbols_name,
                )
            )

    def manifest(self) -> dict:
        return build_manifest(
            repository_root=REPOSITORY_ROOT,
            bindings_path=self.bindings,
            bindings_name="dogmos_bindings.dm",
            artifacts=self.artifacts,
            source_revision="0123456789abcdef0123456789abcdef01234567",
            dirty=False,
        )

    def test_schema_v1_manifest_has_exact_identity_and_paired_artifacts(self) -> None:
        manifest = self.manifest()
        self.assertEqual(manifest["schema_version"], 1)
        self.assertEqual(
            manifest["source_revision"],
            "0123456789abcdef0123456789abcdef01234567",
        )
        self.assertEqual(manifest["build_profile"], "release")
        self.assertEqual(manifest["versions"]["workspace"], "2.3.0")
        self.assertEqual(manifest["versions"]["abi"], 1)
        self.assertEqual(manifest["versions"]["protocol"], 8)
        self.assertEqual(manifest["toolchain"]["rust"], "1.98.0")
        self.assertEqual(manifest["toolchain"]["byond"], "516.1687")
        self.assertEqual(
            manifest["toolchain"]["byondapi_revision"],
            "721dd4690b60954687e41d8691d0040f398d91a6",
        )
        self.assertEqual(
            manifest["capabilities"]["features"],
            sorted(manifest["capabilities"]["features"]),
        )
        self.assertEqual(len(manifest["artifacts"]), 4)
        self.assertEqual(
            {(entry["platform"], entry["role"]) for entry in manifest["artifacts"]},
            {
                ("windows", "shim"),
                ("windows", "service"),
                ("linux", "shim"),
                ("linux", "service"),
            },
        )
        self.assertEqual(
            manifest["bindings"]["sha256"],
            hashlib.sha256(self.bindings.read_bytes()).hexdigest(),
        )

    def test_protocol_7_capability_manifest_is_rejected_for_protocol_8_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            for relative_path in (
                "Cargo.toml",
                "Cargo.lock",
                "rust-toolchain.toml",
                "dogmos-build-manifest.toml",
                "crates/dogmos-protocol/src/lib.rs",
                "crates/dogmos-byond/Cargo.toml",
                "crates/dogmos-server/Cargo.toml",
            ):
                destination = repository / relative_path
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(REPOSITORY_ROOT / relative_path, destination)
            capability = repository / "dogmos-build-manifest.toml"
            capability.write_text(
                capability.read_text(encoding="utf-8").replace(
                    "protocol_version = 8", "protocol_version = 7"
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                ContractError, "capability protocol versions do not match source"
            ):
                build_manifest(
                    repository_root=repository,
                    bindings_path=self.bindings,
                    bindings_name="dogmos_bindings.dm",
                    artifacts=self.artifacts,
                    source_revision="0123456789abcdef0123456789abcdef01234567",
                    dirty=False,
                )

    def test_canonical_json_is_deterministic_sorted_and_has_one_lf(self) -> None:
        first = canonical_manifest_bytes(self.manifest())
        second = canonical_manifest_bytes(self.manifest())
        self.assertEqual(first, second)
        self.assertEqual(
            hashlib.sha256(first).hexdigest(),
            "be7ad56a5203fdc93c8551d1113faa0e078c1c6f360641fa0d9419f838277e0a",
        )
        self.assertTrue(first.endswith(b"\n"))
        self.assertFalse(first.endswith(b"\n\n"))
        self.assertNotIn(b"\r", first)
        self.assertEqual(json.loads(first), self.manifest())

    def test_generation_rejects_development_dirty_and_malformed_revisions(self) -> None:
        for revision, dirty in (
            ("development", False),
            ("0123456789abcdef0123456789abcdef01234567", True),
            ("abc", False),
            ("A123456789abcdef0123456789abcdef01234567", False),
        ):
            with self.subTest(revision=revision, dirty=dirty):
                with self.assertRaises(ContractError):
                    build_manifest(
                        repository_root=REPOSITORY_ROOT,
                        bindings_path=self.bindings,
                        bindings_name="dogmos_bindings.dm",
                        artifacts=self.artifacts,
                        source_revision=revision,
                        dirty=dirty,
                    )

    def test_generation_rejects_wrong_architecture_and_missing_symbols(self) -> None:
        wrong_architecture = list(self.artifacts)
        wrong_architecture[0] = ArtifactInput(
            **{
                **wrong_architecture[0].__dict__,
                "binary_path": wrong_architecture[1].binary_path,
            }
        )
        with self.assertRaises(ContractError):
            build_manifest(
                REPOSITORY_ROOT,
                self.bindings,
                "dogmos_bindings.dm",
                wrong_architecture,
                "0123456789abcdef0123456789abcdef01234567",
                False,
            )
        self.artifacts[0].symbols_path.unlink()
        with self.assertRaises(ContractError):
            self.manifest()

    def test_verification_rejects_changed_hash_and_bindings(self) -> None:
        encoded = canonical_manifest_bytes(self.manifest())
        verify_manifest_bytes(encoded, self.bundle)
        self.artifacts[0].binary_path.write_bytes(b"changed")
        with self.assertRaises(ContractError):
            verify_manifest_bytes(encoded, self.bundle)
        self.artifacts[0].binary_path.write_bytes(pe(0x014C))
        self.bindings.write_bytes(b"changed bindings\n")
        with self.assertRaises(ContractError):
            verify_manifest_bytes(encoded, self.bundle)

    def test_verification_rejects_unsorted_features_and_duplicate_keys(self) -> None:
        manifest = self.manifest()
        manifest["capabilities"]["features"].reverse()
        with self.assertRaises(ContractError):
            verify_manifest_bytes(canonical_manifest_bytes(manifest), self.bundle)
        duplicate = b'{"schema_version":1,"schema_version":1}\n'
        with self.assertRaises(ContractError):
            verify_manifest_bytes(duplicate, self.bundle)

    def test_verification_rejects_crlf_and_terminal_blank_lines(self) -> None:
        encoded = canonical_manifest_bytes(self.manifest())
        with self.assertRaises(ContractError):
            verify_manifest_bytes(encoded.replace(b"\n", b"\r\n"), self.bundle)
        with self.assertRaises(ContractError):
            verify_manifest_bytes(encoded + b"\n", self.bundle)


if __name__ == "__main__":
    unittest.main()
