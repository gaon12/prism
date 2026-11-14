"""No models, network, app launch or original-file writes."""
from pathlib import Path
import importlib.util
import json
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("generation_profile", ROOT / "scripts/prepare-generation-qualification.py")
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class PreparationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / ".dev").mkdir(exist_ok=True)

    def test_real_gate_is_preserved_and_only_the_named_configuration_is_changed(self):
        base = Path(tempfile.mkdtemp(prefix="generation-profile-test-", dir=ROOT / ".dev"))
        self.assertTrue(base.resolve().is_relative_to((ROOT / ".dev").resolve()))
        destination = base / "case"
        manifest = profile.prepare(destination)
        tree = Path(manifest["tree"])
        original = (ROOT / "tests/native/real-generation.mjs").read_text(encoding="utf8")
        generated = (tree / "tests/native/real-generation.mjs").read_text(encoding="utf8")
        reverted = generated.replace(manifest["extraServerVerification"], "")
        for change in reversed(manifest["changes"]):
            reverted = reverted.replace(change["after"], change["before"])
        self.assertEqual(reverted, original)
        for name, digest in manifest["originalInputsUnchanged"].items():
            self.assertEqual(profile.sha(ROOT / name), digest)
        old = json.loads((ROOT / "tests/fixtures/generation/pins.json").read_text())
        new = json.loads((tree / "tests/fixtures/generation/pins.json").read_text())
        self.assertEqual(new["generation"], profile.MODEL)
        self.assertEqual({k: v for k, v in new.items() if k != "generation"}, {k: v for k, v in old.items() if k != "generation"})
        self.assertEqual((tree / "scripts/prepare-generation-assets.mjs").read_bytes(), (ROOT / "scripts/prepare-generation-assets.mjs").read_bytes())
        self.assertEqual(sorted(path.suffix for path in (tree / "tests/fixtures/generation").iterdir()), [".json", ".md", ".md", ".md"])

    def test_a_weakened_original_assertion_is_rejected(self):
        original = (ROOT / "tests/native/real-generation.mjs").read_text(encoding="utf8")
        mutated = original.replace("rows.length >= 3", "rows.length >= 1")
        self.assertNotEqual(mutated, original)
        with self.assertRaisesRegex(ValueError, "Original gate changed"):
            profile.transform(mutated)

    @unittest.skipUnless(shutil.which("node"), "Node is needed for syntax-only checking")
    def test_generated_profile_is_valid_javascript_without_executing_it(self):
        base = Path(tempfile.mkdtemp(prefix="generation-profile-syntax-", dir=ROOT / ".dev"))
        original = ROOT / "tests/native/real-generation.mjs"
        variant = base / "variant.mjs"
        variant.write_text(profile.transform(original.read_text(encoding="utf8")), encoding="utf8")
        for path in [original, variant]:
            checked = subprocess.run([shutil.which("node"), "--check", str(path)], capture_output=True, text=True, timeout=15)
            self.assertEqual(checked.returncode, 0, checked.stderr)

    @unittest.skipUnless(shutil.which("node"), "Node is needed to resolve the screenshot helper")
    def test_generated_screenshot_helper_imports_without_a_browser_or_model(self):
        base = Path(tempfile.mkdtemp(prefix="generation-profile-import-", dir=ROOT / ".dev"))
        manifest = profile.prepare(base / "case")
        helper = Path(manifest["tree"]) / "tests/native/support/document-block-evidence.mjs"
        # Import only the helper. Importing the native runner would execute its
        # entry point, whereas this module merely exports its capture function.
        checked = subprocess.run(
            [shutil.which("node"), "--input-type=module", "-e", "await import(process.argv[1])", helper.as_uri()],
            capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_windows_checkout_line_endings_keep_original_bytes_and_make_same_program(self):
        base = Path(tempfile.mkdtemp(prefix="generation-profile-test-", dir=ROOT / ".dev"))
        reference = profile.prepare(base / "reference")
        fake_root = base / "windows-checkout"
        for name in reference["originalInputsUnchanged"]:
            target = fake_root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        original_path = fake_root / "tests/native/real-generation.mjs"
        windows_bytes = original_path.read_bytes().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
        original_path.write_bytes(windows_bytes)
        with patch.object(profile, "ROOT", fake_root):
            prepared = profile.prepare(fake_root / ".dev" / "case")
        generated_path = Path(prepared["tree"]) / "tests/native/real-generation.mjs"
        reference_path = Path(reference["tree"]) / "tests/native/real-generation.mjs"
        self.assertEqual(generated_path.read_bytes(), reference_path.read_bytes())
        self.assertNotIn(b"\r\n", generated_path.read_bytes())
        self.assertEqual(original_path.read_bytes(), windows_bytes)

    def test_existing_output_and_original_source_are_untouched(self):
        base = Path(tempfile.mkdtemp(prefix="generation-profile-test-", dir=ROOT / ".dev"))
        sentinel = base / "sentinel"
        sentinel.write_text("keep")
        with self.assertRaisesRegex(ValueError, "already exists"):
            profile.prepare(base)
        self.assertEqual(sentinel.read_text(), "keep")
        self.assertEqual(list(base.iterdir()), [sentinel])

    def test_output_outside_private_workspace_is_rejected_before_creation(self):
        with self.assertRaisesRegex(ValueError, "repository .dev"):
            profile.prepare(ROOT / "tests/unused-generation-profile-output")
        self.assertFalse((ROOT / "tests/unused-generation-profile-output").exists())


if __name__ == "__main__":
    unittest.main()
