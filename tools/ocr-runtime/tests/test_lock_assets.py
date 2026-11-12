"""Maintainer inventory tests; no network, package execution or model stand-ins."""
import hashlib
from contextlib import redirect_stdout
import importlib.util
import io
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

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

    def test_macos_arm64_uses_private_unix_python_and_wheel_layout(self):
        self.assertEqual(assets.destination("python", "python/bin/python3.13", "macos-arm64"), "python/bin/python3.13")
        self.assertEqual(assets.destination("wheel", "torch/lib/libtorch.dylib", "macos-arm64"), "python/lib/python3.13/site-packages/torch/lib/libtorch.dylib")
        for scheme in ("purelib", "platlib"):
            self.assertEqual(assets.destination("wheel", f"x.data/{scheme}/mod.py", "macos-arm64"), "python/lib/python3.13/site-packages/mod.py")
        self.assertEqual(assets.destination("wheel", "x.data/scripts/tool", "macos-arm64"), "python/bin/tool")
        self.assertEqual(assets.destination("wheel", "x.data/data/shared.dat", "macos-arm64"), "python/shared.dat")
        self.assertEqual(assets.destination("wheel", "x.data/headers/shared.h", "macos-arm64"), "python/include/shared.h")
        for path in ("outside/file", "../escape", "python/../escape"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                assets.destination("python", path, "macos-arm64")

    def test_macos_rejects_an_unverified_interpreter_before_preparing_files(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            archive = temp / assets.MACOS_PYTHON_NAME
            archive.write_bytes(b"not an interpreter")
            output, stage = temp / "lock.json", temp / "stage"
            arguments = [
                "lock_assets.py", "--target", "macos-arm64",
                "--wheelhouse", str(temp / "absent-wheels"),
                "--python-archive", str(archive),
                "--model-manifest", str(temp / "absent-models.json"),
                "--model-directory", str(temp / "absent-models"),
                "--output", str(output), "--materialize", str(stage),
            ]
            with patch("sys.argv", arguments), self.assertRaisesRegex(ValueError, "unexpected interpreter"):
                assets.main()
            self.assertFalse(output.exists())
            self.assertFalse(stage.exists())


class WheelAuthority(unittest.TestCase):
    def wheel(self, directory, name, version, platform):
        path = directory / f"{name}-{version}-cp313-cp313-{platform}.whl"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr(
                f"{name}-{version}.dist-info/METADATA",
                f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n",
            )
        return path

    def test_macos_torch_uses_pypi_wheel_authority(self):
        for name in ("torch", "torchvision"):
            version = "2.13.0" if name == "torch" else "0.28.0"
            with self.subTest(package=name), tempfile.TemporaryDirectory() as temp:
                wheel = self.wheel(Path(temp), name, version, "macosx_14_0_arm64")
                url = f"https://files.pythonhosted.org/packages/fixture/{wheel.name}"
                metadata = json.dumps({"urls": [{
                    "filename": wheel.name, "size": wheel.stat().st_size,
                    "digests": {"sha256": assets.file_hash(wheel)}, "url": url,
                }]}).encode()

                def fetch(request_url, timeout):
                    self.assertEqual(request_url, f"https://pypi.org/pypi/{name}/{version}/json")
                    return io.BytesIO(metadata)

                with patch.object(assets.urllib.request, "urlopen", fetch):
                    verified = assets.official_wheel(wheel)
                self.assertEqual(verified["url"], url)
                self.assertEqual(verified["sha256"], assets.file_hash(wheel))

    def test_cpu_local_versions_keep_the_pytorch_index_authority(self):
        for platform in ("win_amd64", "manylinux_2_28_x86_64"):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as temp:
                wheel = self.wheel(Path(temp), "torch", "2.13.0+cpu", platform)
                url = "https://download.pytorch.org/whl/cpu/" + assets.urllib.parse.quote(wheel.name)
                page = f'<a href="{url}#sha256={assets.file_hash(wheel)}">wheel</a>'.encode()

                def fetch(request_url, timeout):
                    self.assertEqual(request_url, "https://download.pytorch.org/whl/cpu/torch/")
                    return io.BytesIO(page)

                with patch.object(assets.urllib.request, "urlopen", fetch):
                    verified = assets.official_wheel(wheel)
                self.assertEqual(verified["url"], url)
                self.assertEqual(verified["sha256"], assets.file_hash(wheel))


class ManifestOrder(unittest.TestCase):
    def test_complete_catalog_bytes_do_not_depend_on_host_path_order(self):
        # Preserve real host comparison semantics while reading inert local ZIPs.
        # The Mac qualification failed because Windows sorts PyYAML after aio*,
        # whereas PosixPath sorts that capital P before the lowercase packages.
        class HostPath(type(Path())):
            flavor = PurePosixPath

            def __lt__(self, other):
                return self.flavor(self.name) < self.flavor(other.name)

        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            archive = temp / assets.MACOS_PYTHON_NAME
            with tarfile.open(archive, "w:gz") as output:
                member = tarfile.TarInfo("python/bin/python3.13")
                member.size = 3
                output.addfile(member, io.BytesIO(b"abc"))
            wheels = []
            releases = []
            for name in ("PyYAML", "aiohappyeyeballs"):
                wheel = temp / f"{name}-1.0-py3-none-any.whl"
                with zipfile.ZipFile(wheel, "w") as output:
                    output.writestr(f"{name}.dist-info/METADATA",
                                     f"Metadata-Version: 2.1\nName: {name}\nVersion: 1.0\n")
                wheels.append(HostPath(wheel))
                releases.append({"filename": wheel.name, "size": wheel.stat().st_size,
                                 "digests": {"sha256": assets.file_hash(wheel)},
                                 "url": f"https://fixture.invalid/{wheel.name}"})
            models = temp / "models.json"
            models.write_text('{"files":[]}', encoding="utf-8")
            catalogs = []
            for flavor in (PureWindowsPath, PurePosixPath):
                output = temp / f"{flavor.__name__}.json"
                arguments = ["lock_assets.py", "--target", "macos-arm64",
                             "--wheelhouse", str(temp), "--python-archive", str(archive),
                             "--model-manifest", str(models), "--model-directory", str(temp),
                             "--output", str(output)]
                def fetch(url, timeout):
                    self.assertTrue(url.startswith("https://pypi.org/pypi/"))
                    self.assertEqual(timeout, 60)
                    return io.BytesIO(json.dumps({"urls": releases}).encode())
                with (patch("sys.argv", arguments),
                      patch.object(HostPath, "flavor", flavor),
                      patch.object(Path, "glob", return_value=iter(reversed(wheels))),
                      patch.object(assets, "MACOS_PYTHON_BYTES", archive.stat().st_size),
                      patch.object(assets, "MACOS_PYTHON_SHA256", assets.file_hash(archive)),
                      patch.object(assets.urllib.request, "urlopen", fetch),
                      redirect_stdout(io.StringIO())):
                    assets.main()
                catalogs.append(output.read_bytes())
            self.assertEqual(catalogs[0], catalogs[1])
            manifest = json.loads(catalogs[0])
            self.assertEqual([item["package"] for item in manifest["assets"][1:]],
                             ["aiohappyeyeballs", "PyYAML"])


if __name__ == "__main__":
    unittest.main()
