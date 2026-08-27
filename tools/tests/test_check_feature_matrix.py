import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools" / "check_feature_matrix.ps1"
TARGET = "i686-pc-windows-msvc"
POWERSHELL = shutil.which("pwsh") or shutil.which("powershell.exe")


class FeatureMatrixTests(unittest.TestCase):
	def write_fake_cargo(self, root: Path) -> Path:
		if os.name == "nt":
			fake_cargo = root / "cargo.cmd"
			fake_cargo.write_text(
				"@echo off\n"
				"echo %*>>\"%DOGMOS_FAKE_CARGO_LOG%\"\n"
				"echo %* | findstr /C:\"%DOGMOS_FAKE_CARGO_FAIL_ON%\" >nul\n"
				"if not errorlevel 1 exit /b 17\n"
				"exit /b 0\n",
				encoding="utf-8",
			)
		else:
			fake_cargo = root / "cargo"
			fake_cargo.write_text(
				"#!/bin/sh\n"
				"printf '%s\\n' \"$*\" >> \"$DOGMOS_FAKE_CARGO_LOG\"\n"
				"case \"$*\" in\n"
				"  *\"$DOGMOS_FAKE_CARGO_FAIL_ON\"*) exit 17 ;;\n"
				"esac\n"
				"exit 0\n",
				encoding="utf-8",
			)
			fake_cargo.chmod(0o755)
		return fake_cargo

	def run_matrix(self, fail_on: str = "never-match") -> tuple[subprocess.CompletedProcess[str], list[str]]:
		with tempfile.TemporaryDirectory() as temporary:
			root = Path(temporary)
			log = root / "cargo.log"
			environment = os.environ.copy()
			environment["DOGMOS_FAKE_CARGO_LOG"] = str(log)
			environment["DOGMOS_FAKE_CARGO_FAIL_ON"] = fail_on
			result = subprocess.run(
				[
					POWERSHELL,
					"-NoProfile",
					"-File",
					str(SCRIPT),
					"-CargoPath",
					str(self.write_fake_cargo(root)),
					"-Target",
					TARGET,
				],
				cwd=ROOT,
				env=environment,
				capture_output=True,
				text=True,
			)
			lines = log.read_text(encoding="utf-8").splitlines() if log.exists() else []
			return result, lines

	def test_runs_the_authoritative_supported_matrix(self) -> None:
		self.assertIsNotNone(POWERSHELL)
		result, lines = self.run_matrix()
		self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
		base = f"check --workspace --locked --target {TARGET} --all-targets"
		self.assertEqual(
			lines,
			[
				f"{base} --no-default-features",
				f"{base} --no-default-features --features turf_processing",
				f"{base} --no-default-features --features fastmos",
				f"{base} --no-default-features --features katmos",
				f"{base} --no-default-features --features superconductivity",
				f"{base} --no-default-features --features reaction_hooks",
				f"{base} --no-default-features --features aphelion_reactions",
				f"{base} --no-default-features --features citadel_reactions",
				f"{base} --no-default-features --features yogs_reactions",
				f"{base} --no-default-features --features zas_hooks",
				base,
				f"{base} --features tracy",
			],
		)

	def test_stops_at_the_first_failed_configuration(self) -> None:
		result, lines = self.run_matrix("--features fastmos")
		self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
		self.assertEqual(len(lines), 3)
		self.assertTrue(lines[-1].endswith("--features fastmos"))

	def test_reaction_backends_are_mutually_exclusive(self) -> None:
		result = subprocess.run(
			[
				"cargo",
				"check",
				"--workspace",
				"--locked",
				"--target",
				TARGET,
				"--no-default-features",
				"--features",
				"aphelion_reactions,citadel_reactions",
			],
			cwd=ROOT,
			capture_output=True,
			text=True,
		)
		self.assertNotEqual(result.returncode, 0)
		self.assertIn(
			"only one Dogmos reaction backend can be enabled",
			result.stdout + result.stderr,
		)


if __name__ == "__main__":
	unittest.main()
