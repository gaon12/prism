"""Maintainer inventory tests; no network, package execution or model stand-ins."""
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("lock_assets", Path(__file__).parents[1] / "lock_assets.py")
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)


class InterpreterArchive(unittest.TestCase):
    def fixture(self, archive, links):
        with tarfile.open(archive, "w:gz") as output:
            for name, target in links:
                member = tarfile.TarInfo(name)
                member.type = tarfile.SYMTYPE
                member.linkname = target
                output.addfile(member)
            member = tarfile.TarInfo("python/bin/python3.13")
            member.size = 3
            output.addfile(member, io.BytesIO(b"abc"))

    def test_forward_aliases_are_authenticated_regular_copies(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            archive, root = temp / "python.tar.gz", temp / "stage"
            root.mkdir()
            self.fixture(archive, [("python/bin/python", "python3"), ("python/bin/python3", "python3.13")])
            inventory = []
            assets.unpack_python_tar(archive, lambda *item: inventory.append(item), root)
            self.assertEqual(len(inventory), 3)
            for name, size, sha in inventory:
                self.assertEqual(size, 3)
                self.assertEqual(sha, hashlib.sha256(b"abc").hexdigest())
                self.assertFalse((root / name).is_symlink())
                self.assertEqual((root / name).read_bytes(), b"abc")

    def test_cycles_missing_and_external_targets_are_rejected(self):
        for links in [
            [("python/bin/a", "b"), ("python/bin/b", "a")],
            [("python/bin/a", "../../../escape")],
            [("python/bin/a", "/escape")],
            [("python/bin/a", "missing")],
        ]:
            with self.subTest(links=links), tempfile.TemporaryDirectory() as temp:
                archive = Path(temp) / "python.tar.gz"
                self.fixture(archive, links)
                with self.assertRaises((ValueError, KeyError)):
                    assets.unpack_python_tar(archive, lambda *item: None, None)

    def test_nested_relative_and_hard_aliases_stay_inside_interpreter(self):
        self.assertEqual(assets.alias_target("python/share/1/a", "../a/real", False), "python/share/a/real")
        self.assertEqual(assets.alias_target("python/bin/a", "python/bin/real", True), "python/bin/real")

    def test_wheel_layout_preserves_existing_windows_and_unix_prefixes(self):
        self.assertEqual(assets.destination("wheel", "mod/a.py"), "python/Lib/site-packages/mod/a.py")
        self.assertEqual(assets.destination("wheel", "mod/a.py", "linux-x64"), "python/lib/python3.13/site-packages/mod/a.py")
        self.assertEqual(assets.destination("wheel", "x.data/scripts/tool", "linux-x64"), "python/bin/tool")
        with self.assertRaises(ValueError):
            assets.destination("python", "../escape", "linux-x64")


if __name__ == "__main__":
    unittest.main()
