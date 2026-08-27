import subprocess
import tempfile
import unittest
from pathlib import Path

from tools.check_agent_docs import REQUIRED_GUIDES, check_repository


class AgentDocumentTests(unittest.TestCase):
	def initialize_repository(self, root: Path) -> str:
		subprocess.run(["git", "init", "--quiet", root], check=True)
		subprocess.run(["git", "-C", root, "config", "user.name", "Dogmos Tests"], check=True)
		subprocess.run(["git", "-C", root, "config", "user.email", "dogmos-tests@example.invalid"], check=True)
		(root / "source.txt").write_text("reviewed\n", encoding="utf-8")
		subprocess.run(["git", "-C", root, "add", "source.txt"], check=True)
		subprocess.run(["git", "-C", root, "commit", "--quiet", "-m", "reviewed source"], check=True)
		return subprocess.run(
			["git", "-C", root, "rev-parse", "HEAD"],
			check=True,
			capture_output=True,
			text=True,
		).stdout.strip()

	def write_valid_guidance(self, root: Path, reviewed_revision: str) -> None:
		(root / "docs" / "agent").mkdir(parents=True, exist_ok=True)
		links = "\n".join(f"- [{Path(guide).stem}]({guide})" for guide in REQUIRED_GUIDES)
		(root / "AGENTS.md").write_text(
			"# Dogmos agent instructions\n\n"
			f"{links}\n\n"
			"Protected files require explicit user approval naming the exact file: Cargo.toml, Cargo.lock, "
			"rust-toolchain.toml, .cargo/, .github/workflows, release tooling, artifact tooling, "
			"Docker, and deployment scripts.\n",
			encoding="utf-8",
		)
		guides = {
			"docs/agent/README.md": "# Agent guide index\n",
			"docs/agent/source-authority.md": (
				"# Source authority\n\n"
				f"Reviewed local revision: `{reviewed_revision}`\n\n"
				"Reviewed Auxmos revision: `7757b8eb677796fc3b184768cfe83e91f5b92cba`\n\n"
				"Reviewed on: `2026-08-26`\n"
			),
			"docs/agent/architecture-and-ownership.md": (
				"# Architecture and ownership\n\n"
				"dogmos-byond is the thin shim and is the only crate allowed to depend on byondapi. "
				"dogmosd owns growing simulation state.\n"
			),
			"docs/agent/process-boundary-and-protocol.md": (
				"# Process boundary and protocol\n\n"
				"A Rust DLL allocation remains a DreamDaemon allocation because the DLL is in-process.\n"
			),
			"docs/agent/gameplay-events.md": (
				"# Gameplay events\n\n"
				"Protocol v3 uses a 64-byte event. A fixed buffer holds 1,023 complete events. "
				"Kinds include reaction finished and pressure difference. Do not add a visual-update kind "
				"without its bounded payload. Only DreamDaemon memory is the footprint target.\n"
			),
			"docs/agent/performance-and-memory.md": "# Performance and memory\n",
			"docs/agent/numerical-invariants.md": "# Numerical invariants\n",
			"docs/agent/ffi-and-generated-bindings.md": (
				"# FFI and generated bindings\n\n"
				"Never hand-edit generated bindings or manifests; regenerate and compare them.\n"
			),
			"docs/agent/verification.md": "# Verification\n",
			"docs/agent/release-and-artifacts.md": "# Release and artifacts\n",
			"docs/agent/upstream-drift.md": "# Upstream drift\n",
		}
		for relative, text in guides.items():
			(root / relative).write_text(text, encoding="utf-8")

	def errors_for_valid_fixture(self) -> tuple[Path, list[str]]:
		temporary = tempfile.TemporaryDirectory()
		self.addCleanup(temporary.cleanup)
		root = Path(temporary.name)
		revision = self.initialize_repository(root)
		self.write_valid_guidance(root, revision)
		return root, check_repository(root)

	def test_missing_required_guides_are_reported(self) -> None:
		with tempfile.TemporaryDirectory() as temporary:
			root = Path(temporary)
			self.initialize_repository(root)
			(root / "AGENTS.md").write_text("# Dogmos\n", encoding="utf-8")
			errors = check_repository(root)
			self.assertTrue(any("docs/agent/verification.md" in error for error in errors))

	def test_broken_local_links_are_reported(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		with (root / "docs" / "agent" / "README.md").open("a", encoding="utf-8") as guide:
			guide.write("[missing](missing-guide.md)\n")
		self.assertTrue(any("broken local link" in error for error in check_repository(root)))

	def test_malformed_reviewed_revision_is_reported(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		authority = root / "docs" / "agent" / "source-authority.md"
		authority.write_text(
			authority.read_text(encoding="utf-8").replace(
				"7757b8eb677796fc3b184768cfe83e91f5b92cba",
				"short",
			),
			encoding="utf-8",
		)
		self.assertTrue(any("Reviewed Auxmos revision" in error for error in check_repository(root)))

	def test_unrelated_reviewed_local_revision_is_reported(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		subprocess.run(["git", "-C", root, "checkout", "--quiet", "--orphan", "foreign"], check=True)
		(root / "foreign.txt").write_text("foreign\n", encoding="utf-8")
		subprocess.run(["git", "-C", root, "add", "foreign.txt"], check=True)
		subprocess.run(["git", "-C", root, "commit", "--quiet", "-m", "foreign"], check=True)
		foreign_revision = subprocess.run(
			["git", "-C", root, "rev-parse", "HEAD"],
			check=True,
			capture_output=True,
			text=True,
		).stdout.strip()
		subprocess.run(["git", "-C", root, "checkout", "--quiet", "master"], check=True)
		self.write_valid_guidance(root, foreign_revision)
		self.assertTrue(any("not an ancestor" in error for error in check_repository(root)))

	def test_protected_file_policy_is_required(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		agents = root / "AGENTS.md"
		agents.write_text(agents.read_text(encoding="utf-8").replace("Cargo.toml", "manifest"), encoding="utf-8")
		self.assertTrue(any("protected-file policy" in error for error in check_repository(root)))

	def test_generated_binding_warning_is_required(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		guide = root / "docs" / "agent" / "ffi-and-generated-bindings.md"
		guide.write_text("# FFI and generated bindings\n", encoding="utf-8")
		self.assertTrue(any("generated binding policy" in error for error in check_repository(root)))

	def test_shim_and_service_ownership_is_required(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		guide = root / "docs" / "agent" / "architecture-and-ownership.md"
		guide.write_text("# Architecture and ownership\n", encoding="utf-8")
		self.assertTrue(any("shim/service ownership" in error for error in check_repository(root)))

	def test_in_process_dll_memory_must_be_attributed_to_dreamdaemon(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		guide = root / "docs" / "agent" / "process-boundary-and-protocol.md"
		guide.write_text(
			"# Process boundary and protocol\n\nRust DLL allocations are outside DreamDaemon.\n",
			encoding="utf-8",
		)
		self.assertTrue(any("in-process DLL memory" in error for error in check_repository(root)))

	def test_gameplay_event_contract_is_required(self) -> None:
		root, errors = self.errors_for_valid_fixture()
		self.assertEqual(errors, [])
		guide = root / "docs" / "agent" / "gameplay-events.md"
		guide.write_text("# Gameplay events\n", encoding="utf-8")
		self.assertTrue(any("bounded gameplay event contract" in error for error in check_repository(root)))

	def test_checked_in_repository_documents_are_current(self) -> None:
		root = Path(__file__).resolve().parents[2]
		self.assertEqual(check_repository(root), [])


if __name__ == "__main__":
	unittest.main()
