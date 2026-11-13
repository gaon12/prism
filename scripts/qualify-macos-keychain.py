"""Run the production credential test only on a disposable hosted macOS runner."""

import argparse
import hashlib
import json
import os
import platform
import secrets
import signal
import subprocess
import tempfile
import time
import tomllib
from contextlib import contextmanager
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST = "secret_store::native::tests::native_store_roundtrip_replacement_isolation_and_delete"


class QualificationError(RuntimeError):
    pass


def require_host(system, environment):
    if system != "Darwin" or any(
        environment.get(key) != value
        for key, value in {
            "GITHUB_ACTIONS": "true",
            "RUNNER_ENVIRONMENT": "github-hosted",
            "RUNNER_OS": "macOS",
        }.items()
    ):
        raise QualificationError("Requires a disposable GitHub-hosted macOS runner.")


def keychain_paths(output):
    # Apple's security tool prints one quoted, unescaped absolute path per line.
    # Do not parse these as shell commands or unescape characters in file names.
    paths = []
    for line in output.splitlines():
        line = line.strip()
        if (
            len(line) < 3
            or line[0] != '"'
            or line[-1] != '"'
            or line[1] != "/"
            or "\0" in line
        ):
            raise QualificationError("Unrecognized security keychain path output.")
        paths.append(line[1:-1])
    return paths


class Security:
    def __call__(self, *arguments):
        try:
            result = subprocess.run(
                ["/usr/bin/security", *arguments],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                timeout=20,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            # Never include argv: creation/unlock arguments contain a random password.
            raise QualificationError("Security command could not complete.") from None
        if result.returncode:
            raise QualificationError(f"Security command failed ({result.returncode}).")
        return result.stdout


def snapshot(security):
    default = keychain_paths(security("default-keychain", "-d", "user"))
    search = keychain_paths(security("list-keychains", "-d", "user"))
    if len(default) != 1 or not search:
        raise QualificationError(
            "An existing user default and search list are required."
        )
    return {"default": default[0], "search": search}


@contextmanager
def isolated_keychain(security, parent, report):
    original = snapshot(security)
    report["original"] = original
    owned = (
        Path(tempfile.mkdtemp(prefix="prism-keychain-", dir=parent))
        / "test.keychain-db"
    )
    report["temporaryKeychain"] = str(owned)
    password = secrets.token_hex(32)
    try:
        # Creation itself changes the search list; restore even if it fails halfway.
        security("create-keychain", "-p", password, str(owned))
        if owned.is_symlink() or not owned.is_file():
            raise QualificationError("Temporary keychain was not a regular owned file.")
        security("unlock-keychain", "-p", password, str(owned))
        security("set-keychain-settings", "-lut", "600", str(owned))
        security("default-keychain", "-d", "user", "-s", str(owned))
        security("list-keychains", "-d", "user", "-s", str(owned))
        expected = {"default": str(owned), "search": [str(owned)]}
        if snapshot(security) != expected:
            raise QualificationError(
                "Temporary user keychain isolation was not established."
            )
        report["isolationVerified"] = True
        yield
    finally:
        failures = []
        for arguments in [
            ("default-keychain", "-d", "user", "-s", original["default"]),
            ("list-keychains", "-d", "user", "-s", *original["search"]),
        ]:
            try:
                security(*arguments)
            except QualificationError:
                failures.append(arguments[0])
        try:
            report["restored"] = snapshot(security)
            report["restorationVerified"] = (
                not failures and report["restored"] == original
            )
        except QualificationError:
            report["restorationVerified"] = False
        # Never delete a keychain that might still be the user's selected default.
        if not report["restorationVerified"]:
            raise QualificationError(
                "Original user keychain configuration was not restored."
            )
        if owned.is_file() and not owned.is_symlink():
            security("delete-keychain", str(owned))
        report["temporaryKeychainRemoved"] = not owned.exists()
        if not report["temporaryKeychainRemoved"] or snapshot(security) != original:
            raise QualificationError("Temporary keychain cleanup was not verified.")


def bounded_run(command, output, environment, timeout):
    with (
        output.open("xb") as stdout,
        output.with_suffix(".stderr.log").open("xb") as stderr,
    ):
        child = subprocess.Popen(
            command,
            cwd=ROOT,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
        try:
            code = child.wait(timeout=timeout)
        except BaseException:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
            raise
    if code:
        raise QualificationError(
            f"Qualification child failed ({code}); see its stage log."
        )


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def test_executable(build_log):
    artifacts = []
    for line in build_log.read_text(encoding="utf8").splitlines():
        item = json.loads(line)
        if (
            item.get("reason") == "compiler-artifact"
            and item.get("profile", {}).get("test")
            and item.get("target", {}).get("name") == "prism_rag_qa"
            and item.get("executable")
        ):
            artifacts.append(Path(item["executable"]).resolve(strict=True))
    if len(artifacts) != 1:
        raise QualificationError("Expected one actual RAG QA test executable.")
    return artifacts[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require_host(platform.system(), os.environ)
    temporary_root = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    output = args.output.resolve()
    if not output.is_relative_to(temporary_root) or output == temporary_root:
        raise QualificationError(
            "Evidence must use a new directory inside RUNNER_TEMP."
        )
    output.mkdir(mode=0o700)
    report = {
        "passed": False,
        "test": TEST,
        "host": platform.machine(),
        "sourceCommit": os.environ.get("GITHUB_SHA"),
        "scope": "Actual credential API, not desktop UI.",
    }
    started = time.monotonic()
    try:
        sources = [
            "tools/rag-qa/Cargo.toml",
            "tools/rag-qa/Cargo.lock",
            "tools/rag-qa/src/lib.rs",
            "src-tauri/src/secret_store.rs",
            "src-tauri/src/native_secrets.rs",
            "scripts/qualify-macos-keychain.py",
        ]
        report["inputs"] = {name: sha(ROOT / name) for name in sources}
        identity = lambda p: (
            p["name"],
            p["version"],
            p.get("source"),
            p.get("checksum"),
        )
        app = {
            identity(p)
            for p in tomllib.loads((ROOT / "src-tauri/Cargo.lock").read_text())[
                "package"
            ]
        }
        qa = tomllib.loads((ROOT / "tools/rag-qa/Cargo.lock").read_text())["package"]
        if any(
            identity(p) not in app
            for p in qa
            if p.get("source", "").startswith("registry+")
        ):
            raise QualificationError("QA and application registry pins differ.")
        env = {k: v for k, v in os.environ.items() if not k.startswith("PRISM_")}
        env["CARGO_BUILD_JOBS"] = "1"
        bounded_run(
            [
                "cargo",
                "test",
                "--manifest-path",
                "tools/rag-qa/Cargo.toml",
                "--locked",
                "--lib",
                "--no-run",
                "--message-format=json",
                "-j1",
            ],
            output / "build.jsonl",
            env,
            900,
        )
        executable = test_executable(output / "build.jsonl")
        report["testExecutableSha256"] = sha(executable)
        with isolated_keychain(Security(), temporary_root, report):
            env["PRISM_TEST_OS_CREDENTIALS"] = "1"
            bounded_run(
                [
                    str(executable),
                    TEST,
                    "--exact",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ],
                output / "test.log",
                env,
                60,
            )
            text = (output / "test.log").read_text(encoding="utf8")
            if (
                f"test {TEST} ... ok" not in text
                or "test result: ok. 1 passed; 0 failed; 0 ignored;" not in text
            ):
                raise QualificationError(
                    "The exact native credential test did not pass once."
                )
        if sha(executable) != report["testExecutableSha256"] or any(
            sha(ROOT / name) != digest for name, digest in report["inputs"].items()
        ):
            raise QualificationError("Qualification inputs changed during execution.")
        report["passed"] = True
    except BaseException as error:
        report["failureType"] = type(error).__name__
        raise
    finally:
        report["elapsedSeconds"] = time.monotonic() - started
        (output / "result.json").write_text(
            json.dumps(report, indent=2), encoding="utf8"
        )
        print(
            json.dumps({"passed": report["passed"], "evidence": str(output)}),
            flush=True,
        )


if __name__ == "__main__":
    main()
