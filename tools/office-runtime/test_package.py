"""Maintainer packaging invariants; tiny PE headers are never executed."""

import importlib.util
import os
import struct
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "office_package", Path(__file__).with_name("package_windows.py")
)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


def pe(machine):
    data = bytearray(128)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<H", data, 68, machine)
    return bytes(data)


class Packaging(unittest.TestCase):
    def fixture(self, root):
        source, crt, fonts = root / "extracted", root / "crt", root / "fonts"
        application = source / "application"
        files = {
            "program/soffice.com": pe(0x8664),
            "program/soffice.bin": pe(0x8664),
            "program/pyuno.pyd": pe(0x8664),
            "program/python313.dll": pe(0x8664),
            "program/python-core-3.13.17/bin/python.exe": pe(0x8664),
            "license.txt": b"license",
            "LICENSE.html": b"licenses",
            "NOTICE": b"notice",
            "CREDITS.fodt": b"credits",
        }
        # A flattened MSI directory can contain the last extracted ARM64 DLL.
        files["../System64/vcruntime140.dll"] = pe(0xAA64)
        for name, data in files.items():
            path = application / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (source / "Fonts").mkdir()
        (source / "Fonts/Fallback.ttf").write_bytes(b"font")
        crt.mkdir()
        for name in package.CRT_NAMES:
            (crt / (name + "_amd64.test-id")).write_bytes(pe(0x8664))
        fonts.mkdir()
        (fonts / "Noto-CJK.otf").write_bytes(b"CJK")
        (fonts / "OFL.txt").write_bytes(b"license")
        return application, crt, fonts

    def test_selects_original_amd64_ids_instead_of_flattened_system64(self):
        with tempfile.TemporaryDirectory(prefix="prism-package-") as name:
            root = Path(name)
            source, crt, fonts = self.fixture(root)
            files = package.mapping(source, crt, fonts)
            self.assertEqual(package.machine(files["program/vcruntime140.dll"]), 0x8664)
            self.assertEqual(
                files["program/vcruntime140.dll"],
                crt / "vcruntime140.dll_amd64.test-id",
            )
            self.assertNotIn("program/python-core-3.13.17/bin/python313.dll", files)
            self.assertIn("share/fonts/truetype/Prism-Noto-CJK.otf", files)
            self.assertIn("share/prism-font-notices/OFL.txt", files)

    def test_wrong_architecture_and_incomplete_crt_fail_before_packaging(self):
        with tempfile.TemporaryDirectory(prefix="prism-package-") as name:
            source, crt, fonts = self.fixture(Path(name))
            component = crt / "vcruntime140.dll_amd64.test-id"
            component.write_bytes(pe(0xAA64))
            with self.assertRaises(ValueError):
                package.mapping(source, crt, fonts)
            component.unlink()
            with self.assertRaises(ValueError):
                package.mapping(source, crt, fonts)

    def test_archive_is_reproducible_and_verified_against_exact_inventory(self):
        with tempfile.TemporaryDirectory(prefix="prism-package-") as name:
            root = Path(name)
            files = package.mapping(*self.fixture(root))
            entries, raw = package.inventory(files)
            first, second = root / "a.zip", root / "b.zip"
            package.archive(first, files, raw)
            for path in files.values():
                os.utime(path, (1, 1))
            package.archive(second, files, raw)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            package.verify_archive(first, entries, raw)
            files["license.txt"].write_bytes(b"changed")
            bad = root / "changed.zip"
            package.archive(bad, files, raw)
            with self.assertRaises(ValueError):
                package.verify_archive(bad, entries, raw)

    def test_windows_aliases_and_non_pe_components_are_rejected(self):
        for value in ["../outside", "a//b", "C:/x", "x/CON.txt", "x/file."]:
            with self.assertRaises(ValueError):
                package.relative(value)
        with tempfile.TemporaryDirectory(prefix="prism-package-") as name:
            path = Path(name) / "not-a-dll"
            path.write_bytes(b"text")
            with self.assertRaises(ValueError):
                package.machine(path)


if __name__ == "__main__":
    unittest.main()
