"""Tiny inert source-byte tests; no package parsing, execution or network."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("prepare_sources", Path(__file__).parents[1] / "prepare_sources.py")
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class SourcePreparation(unittest.TestCase):
    def setUp(self):
        self.parent = Path(tempfile.gettempdir()).resolve()
        self.root = Path(tempfile.mkdtemp(prefix="prism-source-unit-")).resolve()
        self.assertEqual(self.root.parent, self.parent)
        self.output = self.root / "prepared"
        self.bodies = {"python.tar.gz": b"inert interpreter bytes", "wheels/module.whl": b"inert wheel bytes", "models/vlm/model.bin": b"inert model bytes"}
        kinds = ["pythonTarGz", "wheel", "model"]
        self.plan = {"schema": 1, "sourceOnly": True, "target": "macos-arm64",
                     "minimumMacOS": "14.0", "pythonVersion": "3.13.18", "pipeline": "PaddleOCR-VL-1.6",
                     "assets": [{"kind": kind, "path": name,
                                 "bytes": len(body), "sha256": hashlib.sha256(body).hexdigest(),
                                 "url": "https://files.pythonhosted.org/fixture/" + name}
                                for kind, (name, body) in zip(kinds, self.bodies.items())]}
        self.requests = []

    def tearDown(self):
        # Delete only this test's unique resolved temporary child.
        self.assertEqual(self.root.resolve(), self.root)
        self.assertEqual(self.root.parent, self.parent)
        self.assertTrue(self.root.name.startswith("prism-source-unit-"))
        shutil.rmtree(self.root)

    def open(self, request, timeout):
        self.requests.append(request.full_url)
        name = request.full_url.split("/fixture/", 1)[1]
        return io.BytesIO(self.bodies[name])

    def run_prepare(self, **options):
        return prepare.prepare(self.plan, self.output, opener=self, **options)

    def status(self):
        return json.loads((self.output / "status.json").read_text(encoding="utf-8"))

    def test_verified_sources_publish_builder_inputs_without_execution(self):
        result = self.run_prepare()
        self.assertTrue(result["prepared"])
        self.assertFalse(result["actualExecution"])
        self.assertEqual(result["verifiedFiles"], 3)
        self.assertEqual(result["verifiedBytes"], sum(map(len, self.bodies.values())))
        self.assertEqual(result, self.status())
        self.assertEqual(len(result["canonicalPlanSha256"]), 64)
        for path, body in self.bodies.items():
            self.assertEqual((self.output / path).read_bytes(), body)
        model = json.loads((self.output / "model-files.json").read_text(encoding="utf-8"))
        self.assertEqual(model["files"][0]["path"], "vlm/model.bin")
        self.assertEqual(model["files"][0]["sha256"], self.plan["assets"][2]["sha256"])
        self.assertEqual(list(self.output.glob(".partial-*")), [])

    def test_reused_models_are_verified_independent_copies(self):
        cached = self.root / "cached"
        source = cached / "vlm/model.bin"
        source.parent.mkdir(parents=True)
        source.write_bytes(self.bodies["models/vlm/model.bin"])
        self.assertTrue(self.run_prepare(reuse_models=cached)["prepared"])
        self.assertEqual(len(self.requests), 2)
        (self.output / "models/vlm/model.bin").write_bytes(b"staged change")
        self.assertEqual(source.read_bytes(), self.bodies["models/vlm/model.bin"])

    def test_existing_output_is_preserved(self):
        self.output.mkdir()
        marker = self.output / "user.txt"
        marker.write_text("preserve")
        with self.assertRaisesRegex(prepare.PreparationError, "directory"):
            self.run_prepare()
        self.assertEqual(marker.read_text(), "preserve")
        self.assertEqual(len(list(self.output.iterdir())), 1)
        self.assertEqual(self.requests, [])

    def test_bad_cached_model_fails_without_downloading_a_replacement(self):
        cached = self.root / "cached"
        source = cached / "vlm/model.bin"
        source.parent.mkdir(parents=True)
        wrong = b"X" * self.plan["assets"][2]["bytes"]
        source.write_bytes(wrong)
        with self.assertRaisesRegex(prepare.PreparationError, "hash"):
            self.run_prepare(reuse_models=cached)
        self.assertEqual(len(self.requests), 2)
        self.assertEqual(source.read_bytes(), wrong)
        self.assertFalse((self.output / "models/vlm/model.bin").exists())
        self.assertFalse((self.output / "model-files.json").exists())

    def test_http_errors_do_not_read_or_publish_the_response_body(self):
        class FailedResponse(io.BytesIO):
            status = 503

            def read(self, *args):
                raise AssertionError("An HTTP error body must not be consumed")

        class FailedRequest:
            def open(self, request, timeout):
                return FailedResponse(b"private upstream error body")

        with self.assertRaisesRegex(prepare.PreparationError, "http"):
            prepare.prepare(self.plan, self.output, opener=FailedRequest())
        self.assertEqual(self.status()["verifiedFiles"], 0)
        self.assertEqual(list(self.output.glob(".partial-*")), [])
        self.assertFalse((self.output / "model-files.json").exists())

    def test_invalid_plan_is_rejected_before_output_or_network(self):
        for key, value in [("path", "../escape.tar.gz"), ("bytes", True),
                           ("sha256", "a" * 63), ("url", "http://files.pythonhosted.org/a"),
                           ("url", "https://user:secret@files.pythonhosted.org/a"),
                           ("url", "https://example.org/a")]:
            with self.subTest(key=key, value=value):
                plan = copy.deepcopy(self.plan)
                plan["assets"][0][key] = value
                with self.assertRaisesRegex(prepare.PreparationError, "manifest"):
                    prepare.prepare(plan, self.output, opener=self)
                self.assertFalse(self.output.exists())
        duplicate = copy.deepcopy(self.plan)
        duplicate["assets"].append(copy.deepcopy(duplicate["assets"][2]))
        duplicate["assets"][-1]["path"] = "models/vlm/MODEL.bin"
        with self.assertRaisesRegex(prepare.PreparationError, "manifest"):
            prepare.prepare(duplicate, self.output, opener=self)
        self.assertFalse(self.output.exists())
        self.assertEqual(self.requests, [])

    def test_hash_failure_preserves_partial_and_never_publishes(self):
        self.bodies["python.tar.gz"] = b"X" * self.plan["assets"][0]["bytes"]
        with self.assertRaisesRegex(prepare.PreparationError, "hash"):
            self.run_prepare()
        self.assertFalse((self.output / "python.tar.gz").exists())
        self.assertEqual(len(list(self.output.glob(".partial-*"))), 1)
        self.assertFalse((self.output / "model-files.json").exists())
        self.assertFalse(self.status()["prepared"])
        self.assertEqual(self.status()["error"]["code"], "hash")

    def test_overlong_body_is_bounded(self):
        self.bodies["python.tar.gz"] += b"one byte too many"
        with self.assertRaisesRegex(prepare.PreparationError, "size"):
            self.run_prepare()
        partial = next(self.output.glob(".partial-*"))
        self.assertLessEqual(partial.stat().st_size, self.plan["assets"][0]["bytes"])
        self.assertFalse((self.output / "python.tar.gz").exists())

    def test_transfer_exception_never_persists_signed_url(self):
        secret = "https://cdn.example.org/data?token=do-not-log-this"

        class Broken:
            def open(self, request, timeout):
                raise RuntimeError(secret)

        with self.assertRaises(prepare.PreparationError) as caught:
            prepare.prepare(self.plan, self.output, opener=Broken())
        self.assertNotIn(secret, str(caught.exception))
        self.assertNotIn(secret, (self.output / "status.json").read_text())
        self.assertEqual(self.status()["error"]["code"], "transfer")
        self.assertFalse((self.output / "model-files.json").exists())

    def test_deadline_prevents_publication(self):
        moments = iter([0, 2])
        with self.assertRaisesRegex(prepare.PreparationError, "timeout"):
            self.run_prepare(timeout=1, clock=lambda: next(moments))
        self.assertEqual(self.status()["verifiedFiles"], 0)
        self.assertFalse((self.output / "python.tar.gz").exists())

    def test_destination_created_during_transfer_is_never_replaced(self):
        owner = self

        class Race:
            def open(self, request, timeout):
                (owner.output / "python.tar.gz").write_bytes(b"preserve")
                return owner.open(request, timeout)

        with self.assertRaises(prepare.PreparationError):
            prepare.prepare(self.plan, self.output, opener=Race())
        self.assertEqual((self.output / "python.tar.gz").read_bytes(), b"preserve")
        self.assertEqual(self.status()["verifiedFiles"], 0)

    def test_redirects_reject_downgrade_and_credentials(self):
        request = prepare.urllib.request.Request("https://github.com/fixture")
        handler = prepare.HttpsRedirects()
        for target in ("http://cdn.example.org/data", "https://user:secret@cdn.example.org/data"):
            with self.subTest(target=target), self.assertRaisesRegex(prepare.PreparationError, "redirect"):
                handler.redirect_request(request, None, 302, "redirect", {}, target)


if __name__ == "__main__":
    unittest.main()
