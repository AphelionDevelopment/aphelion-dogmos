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


if __name__ == "__main__":
	unittest.main()
