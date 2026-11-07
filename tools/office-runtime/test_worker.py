"""Protocol/work-directory tests; actual engine checks are separate and opt-in."""

import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "office_worker", Path(__file__).with_name("worker.py")
)
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class WorkerProtocol(unittest.TestCase):
    def packet(self, **overrides):
        value = {"version": 1, "bytes": 3, "extension": "docx"}
        value.update(overrides)
        return json.dumps(value).encode() + b"\nabc"

    def rejected(self, packet, code):
        with self.assertRaises(worker.WorkerError) as caught:
            worker.read_request(io.BytesIO(packet))
        self.assertEqual(caught.exception.code, code)

    def test_exact_body_leaves_control_byte_unconsumed(self):
        for extension in ["docx", "pptx", "xlsx"]:
            with self.subTest(extension=extension):
                stream = io.BytesIO(self.packet(extension=extension) + b"!")
                self.assertEqual(worker.read_request(stream), (extension, b"abc"))
                self.assertEqual(stream.read(), b"!")

    def test_partial_pipe_reads_preserve_binary_bytes(self):
        class Partial(io.BytesIO):
            def read(self, count):
                return super().read(min(count, 1))

        stream = Partial(b'{"version":1,"extension":"docx","bytes":4}\n\x00\n\xffx')
        self.assertEqual(worker.read_request(stream)[1], b"\x00\n\xffx")

    def test_truncated_header_and_body(self):
        for packet in [b"", b"{}", self.packet()[:-1]]:
            with self.subTest(packet=packet):
                self.rejected(packet, "protocol")

    def test_header_budget_and_invalid_json(self):
        for packet in [b"x" * 4096 + b"\n", b"\xff\n", b"[]\n", b"null\n", b"{bad}\n"]:
            with self.subTest(packet=packet[:10]):
                self.rejected(packet, "protocol")

    def test_protocol_version_is_not_a_boolean_or_coerced_string(self):
        for value in [True, "1", 0, 2, None]:
            self.rejected(self.packet(version=value), "protocol")
        self.rejected(self.packet(extra="file:///not-an-input"), "protocol")

    def test_source_byte_budget_and_input_types(self):
        for size in [0, -1, worker.MAX_BYTES + 1]:
            self.rejected(self.packet(bytes=size), "limit")
        for size in [True, "3", 3.0, None]:
            self.rejected(self.packet(bytes=size), "input")
        for extension in ["docm", "../docx", "DOCX", "file:///input.docx", [], None]:
            self.rejected(self.packet(extension=extension), "input")

    def test_private_directory_requires_ownership_marker(self):
        with tempfile.TemporaryDirectory(prefix="prism-office-protocol-") as name:
            path = Path(name)
            with self.assertRaises(worker.WorkerError):
                worker.work_directory(path)
            (path / "owner.txt").write_bytes(worker.OWNER)
            self.assertEqual(worker.work_directory(path), path.resolve())
            (path / "owner.txt").write_bytes(b"different")
            with self.assertRaises(worker.WorkerError):
                worker.work_directory(path)

    def test_versioned_shared_leases_coexist_and_do_not_upgrade_legacy(self):
        with tempfile.TemporaryDirectory(prefix="prism-office-leases-") as name:
            path = Path(name)
            marker = path / "owner.txt"
            marker.write_bytes(worker.OWNER)
            self.assertIsNone(worker.request_lease(path))
            self.assertEqual(marker.read_bytes(), worker.OWNER)
            marker.write_bytes(worker.LEASED_OWNER)
            self.assertEqual(worker.work_directory(path), path.resolve())
            first = worker.request_lease(path)
            second = worker.request_lease(path)
            try:
                self.assertEqual(marker.read_bytes(), worker.LEASED_OWNER)
            finally:
                second.close()
                first.close()

    def test_bootstrap_replaces_inherited_user_and_configuration_overrides(self):
        with tempfile.TemporaryDirectory(prefix="prism-office-bootstrap-") as name:
            root = Path(name).resolve()
            runtime = root / "engine with spaces $"
            profile = root / "한글 request"
            with patch.dict(
                os.environ,
                {
                    "URE_BOOTSTRAP": "file:///outside/fundamental.ini",
                    "UNO_PATH": "outside",
                    "UserInstallation": "file:///outside/user",
                    "CONFIGURATION_LAYERS": "winreg:CURRENT_USER user:!/outside.xcu",
                },
            ):
                user = worker.configure_bootstrap(runtime, profile)
                self.assertEqual(user, (profile / "user").as_uri())
                self.assertEqual(os.environ["UserInstallation"], user)
                self.assertEqual(os.environ["UNO_PATH"], str(runtime / "program"))
                self.assertEqual(
                    os.environ["URE_BOOTSTRAP"],
                    (
                        runtime
                        / "program"
                        / (
                            "fundamental.ini"
                            if sys.platform == "win32"
                            else "fundamentalrc"
                        )
                    ).as_uri(),
                )
                layers = os.environ["CONFIGURATION_LAYERS"]
                self.assertNotIn("winreg:", layers)
                self.assertNotIn("outside", layers)
                self.assertIn((runtime / "share/registry").as_uri(), layers)
                self.assertIn(f"user:!{user}/user/registrymodifications.xcu", layers)


if __name__ == "__main__":
    unittest.main()
