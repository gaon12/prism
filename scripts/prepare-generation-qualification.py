"""Prepare a separate opt-in model qualification; never download or run a model."""
from pathlib import Path
import argparse
import hashlib
import json
import shutil

ROOT = Path(__file__).resolve().parents[1]
ORIGINAL_TEST_SHA256 = "e9eced744d7ce52ea7504852c586d76d1219d251309b4a08676a5be2d94c5de7"
PROFILE = "qwen3-4b-official-thinking-no-repack-seed42"
MODEL = {
    "filename": "Qwen3-4B-Q4_K_M.gguf",
    "bytes": 2497280256,
    "sha256": "7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5",
    "url": "https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/bc640142c66e1fdd12af0bd68f40445458f3869b/Qwen3-4B-Q4_K_M.gguf",
    "license": "Apache-2.0",
    "testOnly": True,
}
CHANGES = [
    ("            temperature: 0,", "            temperature: 0.6,"),
    ("assert.equal(settings.generation.local.temperature, 0);", "assert.equal(settings.generation.local.temperature, 0.6);"),
    ("      '--no-mmproj',", "      '--no-repack',\n      '--no-mmproj',"),
    ("      '--reasoning',\n      'off',", "      '--reasoning',\n      'on',\n      '--reasoning-format',\n      'deepseek',\n      '--temp',\n      '0.6',\n      '--top-p',\n      '0.95',\n      '--top-k',\n      '20',\n      '--min-p',\n      '0',\n      '--presence-penalty',\n      '1.5',\n      '--seed',\n      '42',"),
    ("  passed: false,", f"  qualificationProfile: '{PROFILE}',\n  passed: false,"),
]
VERIFICATION = """    const qualificationProps = await (
      await fetch(`http://127.0.0.1:${port}/props`, { signal: AbortSignal.timeout(3000) })
    ).json();
    const qualificationSampling = {temperature: 0.6, top_p: 0.95, top_k: 20, min_p: 0, presence_penalty: 1.5, seed: 42};
    for (const [key, expected] of Object.entries(qualificationSampling))
      assert(Math.abs(qualificationProps.default_generation_settings.params[key] - expected) < 1e-5, `Wrong actual server parameter: ${key}`);
    assert.equal(qualificationProps.default_generation_settings.n_ctx, 8192);
    assert.equal(qualificationProps.total_slots, 1);
    result.qualificationSamplingVerified = qualificationSampling;
"""


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def transform(original):
    # read_text normalizes CRLF; pin the canonical committed program on all hosts.
    if hashlib.sha256(original.encode()).hexdigest() != ORIGINAL_TEST_SHA256:
        raise ValueError("Original gate changed; review the profile against it first")
    variant = original
    for before, after in CHANGES:
        if variant.count(before) != 1:
            raise ValueError("Profile anchor is absent or ambiguous")
        variant = variant.replace(before, after)
    marker = "    await instance.invoke('model_start');"
    if variant.count(marker) != 1:
        raise ValueError("Server verification anchor is absent or ambiguous")
    variant = variant.replace(marker, VERIFICATION + marker)
    reverted = variant.replace(VERIFICATION, "")
    for before, after in reversed(CHANGES):
        reverted = reverted.replace(after, before)
    if reverted != original:
        raise ValueError("Profile edits changed more than configuration")
    return variant


def prepare(output):
    output = Path(output).resolve()
    private = (ROOT / ".dev").resolve()
    if not output.is_relative_to(private) or output == private:
        raise ValueError("Output must be a new directory inside repository .dev")
    if output.exists():
        raise ValueError("Output already exists; never replace an earlier case")
    original_path = ROOT / "tests/native/real-generation.mjs"
    original = original_path.read_text(encoding="utf8")
    variant = transform(original)
    sources = [original_path, ROOT / "scripts/prepare-generation-assets.mjs"]
    helpers = ["harness.mjs", "hwpx-inspect.mjs", "support/generation-evidence.mjs", "support/table-headers.mjs", "support/reservation-note.mjs"]
    sources += [ROOT / "tests/native" / name for name in helpers]
    # Captured negative answers in this directory are never qualification inputs.
    fixtures = [ROOT / "tests/fixtures/generation" / name for name in ["pins.json", "은하 워크숍.md", "별빛 세미나.md", "독서 동호회.md"]]
    sources += fixtures
    before = {str(path.relative_to(ROOT)).replace("\\", "/"): sha(path) for path in sources}
    output.mkdir(parents=True)
    tree = output / "tree"
    for source in sources:
        target = tree / source.relative_to(ROOT)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    (tree / "tests/native/real-generation.mjs").write_text(variant, encoding="utf8", newline="\n")
    pins_path = tree / "tests/fixtures/generation/pins.json"
    pins = json.loads(pins_path.read_text(encoding="utf8"))
    pins["generation"] = MODEL
    pins_path.write_text(json.dumps(pins, ensure_ascii=False, indent=2), encoding="utf8")
    if before != {name: sha(ROOT / name) for name in before}:
        raise ValueError("Original input changed during preparation")
    manifest = {
        "profile": PROFILE,
        "scope": "Preparation only. Separate opt-in actual-app qualification; original 1.7B gate and product defaults are unchanged. No inference or download started.",
        "tree": str(tree),
        "originalInputsUnchanged": before,
        "canonicalOriginalProgramSha256": ORIGINAL_TEST_SHA256,
        "reversingConfigurationEditsMatchesOriginal": True,
        "changes": [{"before": a, "after": b} for a, b in CHANGES],
        "extraServerVerification": VERIFICATION,
        "files": {str(path.relative_to(tree)).replace("\\", "/"): sha(path) for path in tree.rglob("*") if path.is_file()},
        "model": MODEL,
        "cases": 1,
        "seed": 42,
        "context": 8192,
        "maxOutputTokensIncludingReasoning": 2048,
        "httpDeadlineSeconds": 180,
        "availableMemoryPreflightGiB": 4,
        "availableMemoryStopGiB": 1,
        "requiredExternalWholeCaseTimeoutMinutes": 15,
        "commands": {
            "prepareAssets": ["node", str(tree / "scripts/prepare-generation-assets.mjs"), str(output / "assets")],
            "verifyAssets": ["node", str(tree / "tests/native/real-generation.mjs"), "--verify-assets"],
            "run": ["node", str(tree / "tests/native/real-generation.mjs")],
        },
    }
    with (output / "qualification-profile.json").open("x", encoding="utf8") as stream:
        json.dump(manifest, stream, ensure_ascii=False, indent=2)
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, help="New directory inside repository .dev")
    args = parser.parse_args()
    try:
        print(json.dumps(prepare(args.output), ensure_ascii=True))
    except (ValueError, OSError) as error:
        parser.exit(2, f"Qualification preparation failed: {error}\n")
