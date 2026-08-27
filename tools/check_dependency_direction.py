#!/usr/bin/env python3
"""Enforce the BYOND-free Dogmos core and wire-protocol boundary."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

BOUNDARY_CRATES = ("dogmos-core", "dogmos-protocol")
DEPENDENCY_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")
FORBIDDEN_SOURCE = (
	(re.compile(r"\bbyondapi\s*::"), "byondapi path"),
	(re.compile(r"\bByondValue\b"), "ByondValue"),
	(re.compile(r"\bcall_global_id\s*\("), "call_global_id"),
	(re.compile(r"\bnew_ref\s*\("), "new_ref"),
)
PUBLIC_USIZE_FIELD = re.compile(r"^\s*pub\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*[^,]*\busize\b")


def dependency_tables(document: dict) -> list[tuple[str, dict]]:
	tables: list[tuple[str, dict]] = []
	for section in DEPENDENCY_SECTIONS:
		value = document.get(section)
		if isinstance(value, dict):
			tables.append((section, value))
	for target_name, target in document.get("target", {}).items():
		if not isinstance(target, dict):
			continue
		for section in DEPENDENCY_SECTIONS:
			value = target.get(section)
			if isinstance(value, dict):
				tables.append((f"target.{target_name}.{section}", value))
	return tables


def check_crate(root: Path, crate_name: str) -> list[str]:
	errors: list[str] = []
	crate = root / "crates" / crate_name
	manifest = crate / "Cargo.toml"
	if not manifest.is_file():
		return [f"missing boundary crate manifest: {manifest.relative_to(root)}"]

	document = tomllib.loads(manifest.read_text(encoding="utf-8"))
	for section, dependencies in dependency_tables(document):
		if "byondapi" in dependencies:
			errors.append(f"{manifest.relative_to(root)}: {section} depends on byondapi")

	source_root = crate / "src"
	for source in sorted(source_root.rglob("*.rs")):
		for line_number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
			code = line.split("//", 1)[0]
			for pattern, label in FORBIDDEN_SOURCE:
				if pattern.search(code):
					errors.append(
						f"{source.relative_to(root)}:{line_number}: forbidden {label} in {crate_name}"
					)
			if PUBLIC_USIZE_FIELD.search(code):
				errors.append(
					f"{source.relative_to(root)}:{line_number}: public boundary field uses usize"
				)
	return errors


def check_repository(root: Path) -> list[str]:
	errors: list[str] = []
	for crate_name in BOUNDARY_CRATES:
		errors.extend(check_crate(root, crate_name))
	return errors


def main() -> int:
	root = Path(__file__).resolve().parents[1]
	errors = check_repository(root)
	for error in errors:
		print(error)
	return 1 if errors else 0


if __name__ == "__main__":
	sys.exit(main())
