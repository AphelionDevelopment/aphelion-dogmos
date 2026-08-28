import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DIRECT_BYOND_ATTRIBUTE = re.compile(r"#\[byondapi::(?:bind|bind_raw_args|init)\b")
GUARDED_ATTRIBUTE = re.compile(r"#\[auxmacros::(?:bind|bind_raw_args|init)\b")


class FfiBoundaryTests(unittest.TestCase):
	def test_all_dogmos_exports_use_the_guarded_boundary(self) -> None:
		sources = list((ROOT / "src").rglob("*.rs"))
		direct = []
		guarded_count = 0
		for source in sources:
			text = source.read_text(encoding="utf-8")
			if DIRECT_BYOND_ATTRIBUTE.search(text):
				direct.append(source.relative_to(ROOT).as_posix())
			guarded_count += len(GUARDED_ATTRIBUTE.findall(text))
		self.assertEqual(direct, [])
		self.assertGreater(guarded_count, 0)

	def test_transport_client_is_separate_from_byond_value_conversion(self) -> None:
		client_source = ROOT / "crates" / "dogmos-byond" / "src" / "client.rs"
		self.assertTrue(client_source.is_file())
		text = client_source.read_text(encoding="utf-8")
		self.assertIn("pub struct DogmosClient", text)
		self.assertIn("pub struct BoundedDogmosClient", text)
		self.assertNotIn("ByondValue", text)

		session_source = ROOT / "crates" / "dogmos-byond" / "src" / "session.rs"
		self.assertTrue(session_source.is_file())
		session_text = session_source.read_text(encoding="utf-8")
		self.assertIn("pub(crate) struct ServiceSession", session_text)
		self.assertNotIn("ByondValue", session_text)


if __name__ == "__main__":
	unittest.main()
