"""Protocol and input boundaries; actual model inference has a Rust opt-in test."""
import hashlib
import http.client
import importlib.util
import json
import pathlib
import threading
import unittest
from http.server import HTTPServer

source = pathlib.Path(__file__).resolve().parents[1] / "worker.py"
spec = importlib.util.spec_from_file_location("prism_ocr_worker", source)
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


def options():
    return dict(kind="image", pages=[1], maxSourcePixels=64000000, maxPixels=8000000,
                maxDimension=3200, maxTextBytes=32768, durationMs=180000)


class Boundaries(unittest.TestCase):
    def test_invalid_metadata_cannot_reach_decoders(self):
        for key, value in [("pages", [0]), ("pages", [True]), ("pages", [2, 1]),
                           ("kind", "https://example.test/private"), ("maxPixels", 0),
                           ("maxDimension", True), ("maxTextBytes", 2**40),
                           ("durationMs", -1), ("maxSourcePixels", float("inf"))]:
            request = options()
            request[key] = value
            with self.subTest(key=key, value=value), self.assertRaises(worker.Failure):
                worker.parameters(request)
        request = options()
        request["path"] = "C:/private.txt"
        with self.assertRaises(worker.Failure):
            worker.parameters(request)

    def test_page_budget_and_order_are_preserved(self):
        value = options()
        value.update(kind="pdf", pages=[1, 3, 200])
        self.assertEqual(worker.parameters(value)["pages"], [1, 3, 200])
        value["pages"] = list(range(1, 202))
        with self.assertRaises(worker.Failure):
            worker.parameters(value)

    def test_fitting_bounds_extreme_images_without_upscaling(self):
        for width, height in [(100000, 1), (1, 100000), (9000, 8000), (30, 20)]:
            w, h = worker.fit(width, height, 3200, 8000000)
            self.assertLessEqual(w * h, 8000000)
            self.assertLessEqual(max(w, h), 3200)
        self.assertEqual(worker.fit(30, 20, 3200, 8000000), (30, 20))
        with self.assertRaises(worker.Failure):
            worker.fit(float("nan"), 10, 3200, 8000000)

    def test_health_proof_and_rejected_requests_never_echo_private_data(self):
        token = "a" * 64
        # No fake OCR response: these requests must be rejected before a model
        # or decoder can be called. A None pipeline makes that boundary explicit.
        server = HTTPServer(("127.0.0.1", 0), worker.handler(None, token))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            client = http.client.HTTPConnection(*server.server_address, timeout=2)
            nonce = "b" * 32
            client.request("GET", "/v1/health?nonce=" + nonce)
            response = client.getresponse()
            self.assertEqual(response.status, 200)
            value = json.loads(response.read())
            self.assertEqual(value["proof"], hashlib.sha256((token + nonce).encode()).hexdigest())
            client.close()
            private = b"private fixture never belongs in errors"
            for auth, expected in [("Bearer wrong", 401), ("Bearer " + token, 422)]:
                client = http.client.HTTPConnection(*server.server_address, timeout=2)
                client.request("POST", "/v1/recognize", b"",
                               {"Authorization": auth, "X-Prism-OCR": json.dumps({"path": private.decode()})})
                response = client.getresponse()
                self.assertEqual(response.status, expected)
                self.assertNotIn(private, response.read())
                client.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
