import tempfile
import unittest
from pathlib import Path

from tools.check_dependency_direction import check_repository


class DependencyDirectionTests(unittest.TestCase):
	def write_crate(self, root: Path, name: str, manifest: str, source: str) -> None:
		crate = root / "crates" / name
		(crate / "src").mkdir(parents=True)
		(crate / "Cargo.toml").write_text(manifest, encoding="utf-8")
		(crate / "src" / "lib.rs").write_text(source, encoding="utf-8")

	def fixture(self) -> Path:
		temporary = tempfile.TemporaryDirectory()
		self.addCleanup(temporary.cleanup)
		root = Path(temporary.name)
		for name in ("dogmos-core", "dogmos-protocol"):
			self.write_crate(
				root,
				name,
				f'[package]\nname = "{name}"\nversion = "0.0.0"\nedition = "2021"\n',
				"pub struct Handle { pub slot: u32, pub generation: u32 }\n",
			)
		return root

	def test_valid_boundary_is_accepted(self) -> None:
		self.assertEqual(check_repository(self.fixture()), [])

	def test_byond_dependency_is_rejected(self) -> None:
		root = self.fixture()
		manifest = root / "crates" / "dogmos-core" / "Cargo.toml"
		manifest.write_text(
			manifest.read_text(encoding="utf-8") + "\n[dependencies]\nbyondapi = \"0.5\"\n",
			encoding="utf-8",
		)
		self.assertTrue(any("depends on byondapi" in error for error in check_repository(root)))

	def test_dm_call_symbols_are_rejected(self) -> None:
		root = self.fixture()
		source = root / "crates" / "dogmos-protocol" / "src" / "lib.rs"
		source.write_text("fn invalid(value: ByondValue) { call_global_id(value); }\n", encoding="utf-8")
		errors = check_repository(root)
		self.assertTrue(any("ByondValue" in error for error in errors))
		self.assertTrue(any("call_global_id" in error for error in errors))

	def test_pointer_sized_public_boundary_fields_are_rejected(self) -> None:
		root = self.fixture()
		source = root / "crates" / "dogmos-core" / "src" / "lib.rs"
		source.write_text("pub struct Handle {\n\tpub slot: usize,\n}\n", encoding="utf-8")
		self.assertTrue(any("public boundary field uses usize" in error for error in check_repository(root)))

	def test_checked_in_repository_obeys_the_boundary(self) -> None:
		root = Path(__file__).resolve().parents[2]
		self.assertEqual(check_repository(root), [])


if __name__ == "__main__":
	unittest.main()
