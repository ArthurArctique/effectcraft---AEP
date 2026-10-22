"""The IR written with -o is UTF-8 even where the system encoding is cp1252 (Windows): names
with emoji or CJK no longer stop the export."""
import tempfile
import unittest
from pathlib import Path

from aep_ir_export import write_json


class Utf8OutputTest(unittest.TestCase):
    def test_ir_file_is_utf8(self):
        text = '{"name":"2M$ le skin le plus cher de CSGO😱 漢字.mp4"}'
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "nested" / "project.json"
            write_json(path, text)
            self.assertEqual(path.read_bytes(), text.encode("utf-8"))


if __name__ == "__main__":
    unittest.main()
