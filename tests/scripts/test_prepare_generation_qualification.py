"""No models, network, app launch or original-file writes."""
from pathlib import Path
import importlib.util
import json
import tempfile
import unittest

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
