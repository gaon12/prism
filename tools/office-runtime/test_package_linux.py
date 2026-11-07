"""Packaging-only ELF/mode/link fixtures; no replacement Office renderer."""

import importlib.util
import os
import stat
import struct
import tempfile
import unittest
from pathlib import Path
from zipfile import ZipFile

spec = importlib.util.spec_from_file_location(
    "package_linux", Path(__file__).with_name("package_linux.py")
)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class LinuxPackage(unittest.TestCase):
    def elf(self, target, machine=62):
        target.parent.mkdir(parents=True, exist_ok=True)
        value = bytearray(20)
        value[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", value, 18, machine)
        target.write_bytes(value)

    def test_elf_machine_is_read_from_the_binary_not_its_name(self):
        with tempfile.TemporaryDirectory(prefix="prism-linux-elf-") as name:
            path = Path(name) / "program"
            self.elf(path)
            self.assertEqual(package.elf_machine(path), 62)
            self.elf(path, 183)
            self.assertEqual(package.elf_machine(path), 183)
            path.write_bytes(b"MZ" + b"\0" * 100)
            with self.assertRaises(ValueError):
                package.elf_machine(path)

    def fixture(self, root):
        runtime, dependencies, fonts = (
            root / "runtime",
            root / "dependencies",
            root / "fonts",
        )
        fonts.mkdir()
        (fonts / "notice.txt").write_bytes(b"owned font notice")
        for name in [
            "program/soffice.bin",
            "program/python.bin",
            "program/pyuno.so",
            "program/libpython3.13.so.1.0",
        ]:
            self.elf(runtime / name)
        for name in ["program/soffice.bin", "program/python.bin"]:
            (runtime / name).chmod(0o755)
        for name in ["LICENSE", "LICENSE.html", "NOTICE", "CREDITS.fodt"]:
            (runtime / name).write_bytes(b"owned notice")
        libraries = dependencies / "usr/lib/x86_64-linux-gnu"
        libraries.mkdir(parents=True)
        self.elf(libraries / "libnss3.so")
        for name in ["libnss3", "libnspr4", "libsqlite3-0"]:
            notice = dependencies / "usr/share/doc" / name / "copyright"
            notice.parent.mkdir(parents=True)
            notice.write_bytes(b"owned dependency notice")
        return runtime, dependencies, fonts

    @unittest.skipUnless(os.name == "posix", "Native Unix permission fixture")
    def test_archive_materializes_links_and_pins_readonly_executable_modes(self):
        with tempfile.TemporaryDirectory(prefix="prism-linux-package-") as name:
            root = Path(name)
            runtime, dependencies, fonts = self.fixture(root)
            (runtime / "program/python-alias").symlink_to("python.bin")
            files = package.mapping(runtime, dependencies, fonts)
            entries, raw = package.inventory(files)
            for entry in entries:
                self.assertIn(entry["unixMode"], [0o444, 0o555])
            modes = {entry["path"]: entry["unixMode"] for entry in entries}
            self.assertEqual(modes["program/python.bin"], 0o555)
            self.assertEqual(modes["NOTICE"], 0o444)
            first, second = root / "one.zip", root / "two.zip"
            package.archive(first, files, entries, raw)
            package.archive(second, files, entries, raw)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with ZipFile(first) as archive:
                self.assertEqual(
                    archive.read("program/python-alias"),
                    archive.read("program/python.bin"),
                )
                self.assertEqual(
                    archive.getinfo("program/python.bin").external_attr >> 16,
                    stat.S_IFREG | 0o555,
                )

    @unittest.skipUnless(os.name == "posix", "Native Unix link fixture")
    def test_links_cannot_escape_the_trusted_source_root(self):
        with tempfile.TemporaryDirectory(prefix="prism-linux-package-") as name:
            root = Path(name)
            runtime, dependencies, fonts = self.fixture(root)
            outside = root / "outside"
            outside.write_bytes(b"do not package")
            (runtime / "program/link").symlink_to(outside)
            with self.assertRaises(ValueError):
                package.mapping(runtime, dependencies, fonts)

    @unittest.skipUnless(os.name == "posix", "Native Unix mode fixture")
    def test_wrong_dependency_architecture_cannot_enter_the_package(self):
        with tempfile.TemporaryDirectory(prefix="prism-linux-package-") as name:
            runtime, dependencies, fonts = self.fixture(Path(name))
            self.elf(dependencies / "usr/lib/x86_64-linux-gnu/libnss3.so", 183)
            with self.assertRaises(ValueError):
                package.mapping(runtime, dependencies, fonts)


if __name__ == "__main__":
    unittest.main()
