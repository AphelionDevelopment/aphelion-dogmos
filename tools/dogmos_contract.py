from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import struct
import subprocess
import tomllib
from typing import Any, Iterable


SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
REVISION_PATTERN = re.compile(r"^[0-9a-f]{40}$")
EXPECTED_ARTIFACTS = {
    ("linux", "service"): ("x86_64-unknown-linux-gnu", "x86_64"),
    ("linux", "shim"): ("i686-unknown-linux-gnu", "i686"),
    ("windows", "service"): ("x86_64-pc-windows-msvc", "x86_64"),
    ("windows", "shim"): ("i686-pc-windows-msvc", "i686"),
}


class ContractError(ValueError):
    pass


@dataclass(frozen=True)
class ArtifactInput:
    platform: str
    role: str
    target: str
    architecture: str
    binary_path: Path
    binary_name: str
    symbols_path: Path
    symbols_name: str


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _read_required(path: Path, description: str) -> bytes:
    if not path.is_file():
        raise ContractError(f"missing {description}: {path}")
    data = path.read_bytes()
    if not data:
        raise ContractError(f"empty {description}: {path}")
    return data


def _safe_bundle_name(name: str, description: str) -> str:
    path = PurePosixPath(name)
    if not name or path.is_absolute() or ".." in path.parts or "\\" in name:
        raise ContractError(f"invalid {description} bundle path: {name!r}")
    normalized = path.as_posix()
    if normalized != name or normalized.startswith("./"):
        raise ContractError(f"noncanonical {description} bundle path: {name!r}")
    return normalized


def _detect_architecture(binary: bytes) -> tuple[str, str]:
    if binary.startswith(b"MZ"):
        if len(binary) < 64:
            raise ContractError("truncated PE artifact")
        pe_offset = struct.unpack_from("<I", binary, 0x3C)[0]
        if pe_offset > len(binary) - 6 or binary[pe_offset : pe_offset + 4] != b"PE\0\0":
            raise ContractError("invalid PE artifact")
        machine = struct.unpack_from("<H", binary, pe_offset + 4)[0]
        architecture = {0x014C: "i686", 0x8664: "x86_64"}.get(machine)
        if architecture is None:
            raise ContractError(f"unsupported PE machine 0x{machine:04x}")
        return "pe", architecture
    if binary.startswith(b"\x7fELF"):
        if len(binary) < 20:
            raise ContractError("truncated ELF artifact")
        if binary[5] != 1:
            raise ContractError("only little-endian ELF artifacts are supported")
        elf_class = binary[4]
        machine = struct.unpack_from("<H", binary, 18)[0]
        architecture = {(1, 3): "i686", (2, 62): "x86_64"}.get(
            (elf_class, machine)
        )
        if architecture is None:
            raise ContractError(
                f"unsupported ELF class/machine combination {elf_class}/{machine}"
            )
        return "elf", architecture
    raise ContractError("artifact is neither PE nor ELF")


def _protocol_versions(repository_root: Path) -> tuple[int, int]:
    source = _read_required(
        repository_root / "crates" / "dogmos-protocol" / "src" / "lib.rs",
        "protocol source",
    ).decode("utf-8")
    abi = re.search(r"DOGMOS_ABI_VERSION: u16 = (\d+)", source)
    protocol = re.search(r"DOGMOS_PROTOCOL_VERSION: u16 = (\d+)", source)
    if abi is None or protocol is None:
        raise ContractError("protocol source does not declare ABI and protocol versions")
    return int(abi.group(1)), int(protocol.group(1))


def _package_version(path: Path) -> str:
    document = tomllib.loads(_read_required(path, "Cargo manifest").decode("utf-8"))
    version = document.get("package", {}).get("version")
    if not isinstance(version, str) or not version:
        raise ContractError(f"Cargo manifest has no package version: {path}")
    return version


def _byondapi_revision(repository_root: Path) -> str:
    lock = _read_required(repository_root / "Cargo.lock", "Cargo lockfile").decode(
        "utf-8"
    )
    match = re.search(
        r'name = "byondapi".*?source = "git\+[^"#]+(?:\?[^"#]+)?#([0-9a-f]{40})"',
        lock,
        re.DOTALL,
    )
    if match is None:
        raise ContractError("Cargo.lock has no exact byondapi Git revision")
    return match.group(1)


def _source_metadata(repository_root: Path) -> dict[str, Any]:
    capability_path = repository_root / "dogmos-build-manifest.toml"
    capability_bytes = _read_required(capability_path, "capability manifest")
    capability = tomllib.loads(capability_bytes.decode("utf-8"))
    features = capability.get("capabilities", {}).get("features")
    if not isinstance(features, list) or not all(
        isinstance(feature, str) and feature for feature in features
    ):
        raise ContractError("capability manifest has invalid features")
    if features != sorted(set(features)):
        raise ContractError("capability features must be sorted and unique")
    workspace_manifest = tomllib.loads(
        _read_required(repository_root / "Cargo.toml", "workspace Cargo manifest").decode(
            "utf-8"
        )
    )
    default_features = workspace_manifest.get("features", {}).get("default")
    if not isinstance(default_features, list) or sorted(default_features) != features:
        raise ContractError("capability features do not match Cargo defaults")
    rust_toolchain = tomllib.loads(
        _read_required(repository_root / "rust-toolchain.toml", "Rust toolchain").decode(
            "utf-8"
        )
    )
    rust_channel = rust_toolchain.get("toolchain", {}).get("channel")
    if not isinstance(rust_channel, str) or not rust_channel:
        raise ContractError("Rust toolchain has no exact channel")
    byond_version = capability.get("toolchain", {}).get("byond")
    if not isinstance(byond_version, str) or not re.fullmatch(
        r"\d+\.\d+", byond_version
    ):
        raise ContractError("capability manifest has no exact BYOND version")
    abi_version, protocol_version = _protocol_versions(repository_root)
    if capability.get("protocol") != {
        "abi_version": abi_version,
        "protocol_version": protocol_version,
    }:
        raise ContractError("capability protocol versions do not match source")
    return {
        "versions": {
            "abi": abi_version,
            "dogmos-byond": _package_version(
                repository_root / "crates" / "dogmos-byond" / "Cargo.toml"
            ),
            "dogmos-server": _package_version(
                repository_root / "crates" / "dogmos-server" / "Cargo.toml"
            ),
            "protocol": protocol_version,
            "workspace": _package_version(repository_root / "Cargo.toml"),
        },
        "toolchain": {
            "byond": byond_version,
            "byondapi_revision": _byondapi_revision(repository_root),
            "rust": rust_channel,
        },
        "capabilities": {
            "feature_fingerprint": _sha256(capability_bytes),
            "features": features,
        },
    }


def build_manifest(
    repository_root: Path,
    bindings_path: Path,
    bindings_name: str,
    artifacts: Iterable[ArtifactInput],
    source_revision: str,
    dirty: bool,
) -> dict[str, Any]:
    repository_root = Path(repository_root)
    if dirty:
        raise ContractError("refusing to generate a release contract from dirty source")
    if not REVISION_PATTERN.fullmatch(source_revision):
        raise ContractError("source revision must be exact lowercase 40-hex")
    bindings_name = _safe_bundle_name(bindings_name, "bindings")
    bindings = _read_required(Path(bindings_path), "generated bindings")
    entries = []
    seen_pairs = set()
    seen_names = {bindings_name}
    for artifact in artifacts:
        pair = (artifact.platform, artifact.role)
        expected = EXPECTED_ARTIFACTS.get(pair)
        if expected is None or pair in seen_pairs:
            raise ContractError(f"unexpected or duplicate artifact pair: {pair}")
        seen_pairs.add(pair)
        expected_target, expected_architecture = expected
        if (artifact.target, artifact.architecture) != (
            expected_target,
            expected_architecture,
        ):
            raise ContractError(f"artifact identity does not match {pair}")
        binary_name = _safe_bundle_name(artifact.binary_name, "artifact")
        symbols_name = _safe_bundle_name(artifact.symbols_name, "symbols")
        if binary_name in seen_names or symbols_name in seen_names:
            raise ContractError("bundle paths must be unique")
        seen_names.update((binary_name, symbols_name))
        binary = _read_required(Path(artifact.binary_path), "artifact")
        symbols = _read_required(Path(artifact.symbols_path), "symbols")
        artifact_format, detected_architecture = _detect_architecture(binary)
        expected_format = "pe" if artifact.platform == "windows" else "elf"
        if artifact_format != expected_format or detected_architecture != artifact.architecture:
            raise ContractError(
                f"artifact bytes do not match {artifact.platform}/{artifact.architecture}"
            )
        entries.append(
            {
                "architecture": artifact.architecture,
                "file": binary_name,
                "format": artifact_format,
                "platform": artifact.platform,
                "role": artifact.role,
                "sha256": _sha256(binary),
                "size": len(binary),
                "symbols": {
                    "file": symbols_name,
                    "sha256": _sha256(symbols),
                    "size": len(symbols),
                },
                "target": artifact.target,
            }
        )
    if seen_pairs != set(EXPECTED_ARTIFACTS):
        raise ContractError("release contract requires all four platform/role pairs")
    entries.sort(key=lambda entry: (entry["platform"], entry["role"]))
    source = _source_metadata(repository_root)
    return {
        "artifacts": entries,
        "bindings": {
            "file": bindings_name,
            "sha256": _sha256(bindings),
            "size": len(bindings),
        },
        "build_profile": "release",
        "capabilities": source["capabilities"],
        "schema_version": 1,
        "source_revision": source_revision,
        "toolchain": source["toolchain"],
        "versions": source["versions"],
    }


def canonical_manifest_bytes(manifest: dict[str, Any]) -> bytes:
    return (
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ContractError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _verify_file(record: dict[str, Any], bundle_root: Path, description: str) -> bytes:
    if not isinstance(record, dict):
        raise ContractError(f"invalid {description} record")
    name = _safe_bundle_name(record.get("file", ""), description)
    data = _read_required(bundle_root / PurePosixPath(name), description)
    if record.get("size") != len(data):
        raise ContractError(f"{description} size mismatch: {name}")
    digest = record.get("sha256")
    if not isinstance(digest, str) or not SHA256_PATTERN.fullmatch(digest):
        raise ContractError(f"invalid {description} digest: {name}")
    if digest != _sha256(data):
        raise ContractError(f"{description} digest mismatch: {name}")
    return data


def verify_manifest_bytes(data: bytes, bundle_root: Path) -> dict[str, Any]:
    if b"\r" in data or not data.endswith(b"\n") or data.endswith(b"\n\n"):
        raise ContractError("manifest must use LF and end with exactly one terminal LF")
    try:
        manifest = json.loads(data.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ContractError(f"invalid manifest JSON: {error}") from error
    if not isinstance(manifest, dict) or canonical_manifest_bytes(manifest) != data:
        raise ContractError("manifest JSON is not canonical")
    if manifest.get("schema_version") != 1:
        raise ContractError("unsupported release contract schema")
    if manifest.get("build_profile") != "release":
        raise ContractError("release contract build profile must be release")
    revision = manifest.get("source_revision")
    if not isinstance(revision, str) or not REVISION_PATTERN.fullmatch(revision):
        raise ContractError("release contract has an invalid source revision")
    capabilities = manifest.get("capabilities")
    if not isinstance(capabilities, dict):
        raise ContractError("release contract has no capabilities")
    features = capabilities.get("features")
    if not isinstance(features, list) or features != sorted(set(features)):
        raise ContractError("release contract features must be sorted and unique")
    fingerprint = capabilities.get("feature_fingerprint")
    if not isinstance(fingerprint, str) or not SHA256_PATTERN.fullmatch(fingerprint):
        raise ContractError("release contract has an invalid feature fingerprint")
    toolchain = manifest.get("toolchain")
    if not isinstance(toolchain, dict):
        raise ContractError("release contract has no toolchain identity")
    if not re.fullmatch(r"\d+\.\d+\.\d+", toolchain.get("rust", "")):
        raise ContractError("release contract has an invalid Rust version")
    if not re.fullmatch(r"\d+\.\d+", toolchain.get("byond", "")):
        raise ContractError("release contract has an invalid BYOND version")
    if not REVISION_PATTERN.fullmatch(toolchain.get("byondapi_revision", "")):
        raise ContractError("release contract has an invalid byondapi revision")
    versions = manifest.get("versions")
    if not isinstance(versions, dict) or set(versions) != {
        "abi",
        "dogmos-byond",
        "dogmos-server",
        "protocol",
        "workspace",
    }:
        raise ContractError("release contract has invalid version fields")
    if not isinstance(versions["abi"], int) or not isinstance(
        versions["protocol"], int
    ):
        raise ContractError("release contract protocol versions must be integers")
    for package in ("dogmos-byond", "dogmos-server", "workspace"):
        if not re.fullmatch(r"\d+\.\d+\.\d+", versions[package]):
            raise ContractError(f"release contract has invalid {package} version")
    bindings = manifest.get("bindings")
    _verify_file(bindings, Path(bundle_root), "bindings")
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, list) or len(artifacts) != len(EXPECTED_ARTIFACTS):
        raise ContractError("release contract requires exactly four artifacts")
    pairs = []
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise ContractError("invalid artifact record")
        pair = (artifact.get("platform"), artifact.get("role"))
        expected = EXPECTED_ARTIFACTS.get(pair)
        if expected is None or pair in pairs:
            raise ContractError(f"unexpected or duplicate artifact pair: {pair}")
        pairs.append(pair)
        expected_target, expected_architecture = expected
        if artifact.get("target") != expected_target:
            raise ContractError(f"artifact target mismatch: {pair}")
        if artifact.get("architecture") != expected_architecture:
            raise ContractError(f"artifact architecture mismatch: {pair}")
        binary = _verify_file(artifact, Path(bundle_root), "artifact")
        artifact_format, architecture = _detect_architecture(binary)
        if artifact.get("format") != artifact_format or architecture != expected_architecture:
            raise ContractError(f"artifact byte architecture mismatch: {pair}")
        _verify_file(artifact.get("symbols"), Path(bundle_root), "symbols")
    if pairs != sorted(EXPECTED_ARTIFACTS):
        raise ContractError("artifact records must use canonical platform/role order")
    return manifest


def _git_identity(repository_root: Path) -> tuple[str, bool]:
    revision = subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD"],
        cwd=repository_root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    status = subprocess.run(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=repository_root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return revision, bool(status)


def _cli_artifacts(arguments: argparse.Namespace) -> list[ArtifactInput]:
    specifications = (
        (
            "windows",
            "shim",
            "i686-pc-windows-msvc",
            "i686",
            arguments.windows_shim,
            "windows/dogmos.dll",
            arguments.windows_shim_symbols,
            "windows/dogmos.pdb",
        ),
        (
            "windows",
            "service",
            "x86_64-pc-windows-msvc",
            "x86_64",
            arguments.windows_service,
            "windows/dogmosd.exe",
            arguments.windows_service_symbols,
            "windows/dogmosd.pdb",
        ),
        (
            "linux",
            "shim",
            "i686-unknown-linux-gnu",
            "i686",
            arguments.linux_shim,
            "linux/libdogmos.so",
            arguments.linux_shim_symbols,
            "linux/libdogmos.so.debug",
        ),
        (
            "linux",
            "service",
            "x86_64-unknown-linux-gnu",
            "x86_64",
            arguments.linux_service,
            "linux/dogmosd",
            arguments.linux_service_symbols,
            "linux/dogmosd.debug",
        ),
    )
    return [ArtifactInput(*specification) for specification in specifications]


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Generate or verify a Dogmos release contract")
    subparsers = parser.add_subparsers(dest="command", required=True)
    generate = subparsers.add_parser("generate")
    generate.add_argument("--repository-root", type=Path, required=True)
    generate.add_argument("--bindings", type=Path, required=True)
    generate.add_argument("--output", type=Path, required=True)
    for name in (
        "windows-shim",
        "windows-shim-symbols",
        "windows-service",
        "windows-service-symbols",
        "linux-shim",
        "linux-shim-symbols",
        "linux-service",
        "linux-service-symbols",
    ):
        generate.add_argument(f"--{name}", dest=name.replace("-", "_"), type=Path, required=True)
    verify = subparsers.add_parser("verify")
    verify.add_argument("--manifest", type=Path, required=True)
    verify.add_argument("--bundle-root", type=Path, required=True)
    return parser


def main() -> int:
    arguments = _parser().parse_args()
    try:
        if arguments.command == "verify":
            verify_manifest_bytes(arguments.manifest.read_bytes(), arguments.bundle_root)
            return 0
        repository_root = arguments.repository_root.resolve()
        revision, dirty = _git_identity(repository_root)
        manifest = build_manifest(
            repository_root,
            arguments.bindings,
            "dogmos_bindings.dm",
            _cli_artifacts(arguments),
            revision,
            dirty,
        )
        arguments.output.write_bytes(canonical_manifest_bytes(manifest))
        return 0
    except (ContractError, OSError, subprocess.CalledProcessError) as error:
        print(f"dogmos contract error: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
