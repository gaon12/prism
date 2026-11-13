"""Run the explicit synthetic filename CPU benchmark in verified Windows Jobs."""

import argparse
import ctypes
import hashlib
import json
import os
import queue
import subprocess
import threading
import time
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST = "index::service::benchmark::cpu::synthetic_preparation_cpu_stages"
CASES = [
    ("fused-a", "fused"),
    ("stages-a", "stages"),
    ("stages-b", "stages"),
    ("fused-b", "fused"),
]
RECORDS = 2_200_000


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class Memory(ctypes.Structure):
    _fields_ = [("length", ctypes.c_ulong), ("load", ctypes.c_ulong)] + [
        (name, ctypes.c_ulonglong)
        for name in [
            "total",
            "available",
            "page_total",
            "page_available",
            "virtual_total",
            "virtual_available",
            "extended",
        ]
    ]


def available():
    memory = Memory()
    memory.length = ctypes.sizeof(memory)
    if not ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(memory)):
        raise RuntimeError("Memory preflight failed.")
    return memory.available


def stop_tree(process):
    if process.poll() is None:
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            capture_output=True,
            creationflags=subprocess.CREATE_NO_WINDOW,
            check=False,
        )
        process.wait(timeout=15)


def logged_lines(process, path):
    messages = queue.Queue()

    def copy():
        with path.open("x", encoding="utf8") as output:
            for line in process.stdout:
                output.write(line)
                output.flush()
                messages.put(line)
        messages.put(None)

    threading.Thread(target=copy, daemon=True).start()
    return messages


def next_object(messages, timeout=30):
    deadline = time.monotonic() + timeout
    while True:
        line = messages.get(timeout=max(0.01, deadline - time.monotonic()))
        if line is None:
            raise RuntimeError("Owned process exited before its expected JSON result.")
        if line.lstrip().startswith("{"):
            return json.loads(line)
        if time.monotonic() >= deadline:
            raise TimeoutError("Owned process did not report its expected JSON result.")


def drain(messages):
    while messages.get(timeout=30) is not None:
        pass


def source_inputs():
    paths = [
        ROOT / "scripts/profile-filename-cpu.py",
        ROOT / "src-tauri/Cargo.lock",
        ROOT / "src-tauri/src/index.rs",
        ROOT / "src-tauri/src/keyboard_pattern.rs",
        ROOT / "indexer/core/examples/support/captured_rows.rs",
    ]
    paths += list((ROOT / "src-tauri/src").glob("filename_*.rs"))
    paths += list((ROOT / "tools/filename-qa/src").glob("*.rs"))
    paths += [
        ROOT / "tools/filename-qa" / name
        for name in ["build.rs", "Cargo.toml", "Cargo.lock"]
    ]
    for crate in ["core", "protocol", "windows"]:
        paths.append(ROOT / f"indexer/{crate}/Cargo.toml")
        paths += list((ROOT / f"indexer/{crate}/src").rglob("*.rs"))
    paths.append(ROOT / "tests/native/support/job-limit.ps1")
    return {str(path.relative_to(ROOT)).replace("\\", "/"): sha(path) for path in paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if os.name != "nt":
        raise RuntimeError(
            "This profile requires Windows Job Objects and thread CPU clocks."
        )
    output = args.output.resolve()
    allowed = [(ROOT / ".dev").resolve()]
    if os.environ.get("RUNNER_TEMP"):
        allowed.append(Path(os.environ["RUNNER_TEMP"]).resolve())
    if not any(
        output != parent and output.is_relative_to(parent) for parent in allowed
    ):
        raise RuntimeError("Use a new evidence directory in .dev or RUNNER_TEMP.")
    output.mkdir(mode=0o700)
    report = {
        "passed": False,
        "records": RECORDS,
        "cpus": 1,
        "memoryMB": 512,
        "test": TEST,
        "cases": [],
        "scope": "Synthetic codec/preparation/publication CPU; no IPC, GUI, live volume or service.",
    }
    active, finished = [], threading.Event()
    report["initialAvailable"] = available()
    report["minimumAvailable"] = report["initialAvailable"]
    report["resourceStop"] = False
    report["timeStop"] = False
    started = time.monotonic()

    def monitor():
        while not finished.wait(0.1):
            report["minimumAvailable"] = min(report["minimumAvailable"], available())
            report["resourceStop"] = report["minimumAvailable"] < 1024**3
            report["timeStop"] = time.monotonic() - started > 1200
            if report["resourceStop"] or report["timeStop"]:
                for process in active:
                    stop_tree(process)
                break

    try:
        if report["initialAvailable"] < 3 * 1024**3:
            raise RuntimeError(
                "Filename CPU profile requires 3 GiB available before starting."
            )
        report["inputs"] = source_inputs()
        identity = lambda item: (
            item["name"],
            item["version"],
            item.get("source"),
            item.get("checksum"),
        )
        app = {
            identity(item)
            for item in tomllib.loads(
                (ROOT / "src-tauri/Cargo.lock").read_text(encoding="utf8")
            )["package"]
        }
        qa = tomllib.loads(
            (ROOT / "tools/filename-qa/Cargo.lock").read_text(encoding="utf8")
        )["package"]
        if any(
            identity(item) not in app
            for item in qa
            if item.get("source", "").startswith("registry+")
        ):
            raise RuntimeError("Filename QA registry pins differ from the application.")
        threading.Thread(target=monitor, daemon=True).start()
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("PRISM_")
        }
        env["CARGO_BUILD_JOBS"] = "1"
        report["compiler"] = subprocess.run(
            ["rustc", "--version"], check=True, capture_output=True, text=True
        ).stdout.strip()
        report["sourceCommit"] = (
            os.environ.get("GITHUB_SHA")
            or subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=ROOT,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
        )
        command = [
            "cargo",
            "test",
            "--manifest-path",
            "tools/filename-qa/Cargo.toml",
            "--locked",
            "--release",
            "--lib",
            "--no-run",
            "--message-format=json",
            "-j1",
        ]
        with (
            (output / "build.jsonl").open("xb") as log,
            (output / "build.stderr.log").open("xb") as error,
        ):
            build = subprocess.Popen(
                command,
                cwd=ROOT,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=error,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            active.append(build)
            if build.wait(timeout=900):
                raise RuntimeError(
                    "CPU benchmark compilation failed; inspect the build log."
                )
        artifacts = []
        for line in (output / "build.jsonl").read_text(encoding="utf8").splitlines():
            item = json.loads(line)
            if (
                item.get("reason") == "compiler-artifact"
                and item.get("profile", {}).get("test")
                and item.get("target", {}).get("name") == "prism_filename_qa"
                and item.get("executable")
            ):
                artifacts.append(Path(item["executable"]).resolve(strict=True))
        if len(artifacts) != 1:
            raise RuntimeError("Expected exactly one current filename test executable.")
        executable = artifacts[0]
        report["testExecutableSha256"] = sha(executable)
        inventory = subprocess.run(
            [str(executable), TEST, "--exact", "--ignored", "--list"],
            cwd=ROOT,
            env=env,
            check=True,
            capture_output=True,
            text=True,
            timeout=15,
            creationflags=subprocess.CREATE_NO_WINDOW,
        ).stdout
        (output / "selected-test.txt").write_text(inventory, encoding="utf8")
        if f"{TEST}: test" not in inventory.splitlines():
            raise RuntimeError(
                "The current executable does not contain the exact CPU test."
            )
        helper_source = ROOT / "tests/native/support/job-limit.ps1"
        expected = None
        for name, mode in CASES:
            if report["resourceStop"] or report["timeStop"]:
                raise RuntimeError(
                    "Profile resource/time guard stopped before this case."
                )
            with (
                (output / f"{name}-helper.stderr.log").open(
                    "x", encoding="utf8"
                ) as helper_error,
                (output / f"{name}.stderr.log").open(
                    "x", encoding="utf8"
                ) as child_error,
            ):
                helper = subprocess.Popen(
                    [
                        "pwsh",
                        "-NoProfile",
                        "-File",
                        str(helper_source),
                        "-Cpus",
                        "1",
                        "-MemoryMB",
                        "512",
                    ],
                    cwd=ROOT,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=helper_error,
                    text=True,
                    encoding="utf8",
                    creationflags=subprocess.CREATE_NO_WINDOW,
                )
                active.append(helper)
                hq = logged_lines(helper, output / f"{name}-helper.log")
                if not next_object(hq).get("prepared"):
                    raise RuntimeError("Owned Job helper was not prepared.")
                child_env = {
                    **env,
                    "PRISM_FILENAME_CPU_START_GATE": "1",
                    "PRISM_FILENAME_CPU_RECORDS": str(RECORDS),
                    "PRISM_FILENAME_CPU_MODE": mode,
                }
                child = subprocess.Popen(
                    [
                        str(executable),
                        TEST,
                        "--exact",
                        "--ignored",
                        "--nocapture",
                        "--test-threads=1",
                    ],
                    cwd=ROOT,
                    env=child_env,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=child_error,
                    text=True,
                    encoding="utf8",
                    creationflags=subprocess.CREATE_NO_WINDOW,
                )
                active.append(child)
                cq = logged_lines(child, output / f"{name}.log")
                if next_object(cq) != {"waitingForStart": True}:
                    raise RuntimeError("Benchmark did not wait for the start gate.")
                helper.stdin.write(f"assign {child.pid}\n")
                helper.stdin.flush()
                limits = next_object(hq)
                if not (
                    limits.get("ready")
                    and limits["limitFlags"] == 0x310
                    and int(limits["affinityMask"]).bit_count() == 1
                    and child.pid in limits["processIds"]
                    and limits["processMemoryLimitBytes"]
                    == limits["jobMemoryLimitBytes"]
                    == 512 * 1024**2
                ):
                    raise RuntimeError(
                        "Kernel Job limits were not verified before the gate."
                    )
                if report["resourceStop"] or report["timeStop"]:
                    raise RuntimeError(
                        "Profile guard stopped before releasing the gate."
                    )
                child.stdin.write("G")
                child.stdin.flush()
                child.stdin.close()
                if child.wait(timeout=240):
                    raise RuntimeError("CPU benchmark failed; inspect its stage log.")
                value = next_object(cq)
                helper.stdin.write("report\n")
                helper.stdin.flush()
                job = next_object(hq)
                helper.stdin.write("exit\n")
                helper.stdin.flush()
                helper.stdin.close()
                if (
                    helper.wait(timeout=15)
                    or job["totalProcesses"] != 1
                    or job["activeProcesses"] != 0
                ):
                    raise RuntimeError(
                        "Owned benchmark process cleanup was not verified."
                    )
                drain(cq)
                drain(hq)
                if "test result: ok. 1 passed; 0 failed; 0 ignored;" not in (
                    output / f"{name}.log"
                ).read_text(encoding="utf8"):
                    raise RuntimeError("The exact ignored benchmark did not pass once.")
                if not (
                    value["synthetic"]
                    and value["records"] == RECORDS
                    and value["workers"] == 1
                    and value["mode"] == mode
                    and value["realProvisionalSwap"]
                    and value["storedChecksumBefore"] == value["storedChecksumAfter"]
                ):
                    raise RuntimeError(
                        "Synthetic record/publication invariants failed."
                    )
                if expected is None:
                    expected = value["storedChecksumAfter"]
                if expected != value["storedChecksumAfter"]:
                    raise RuntimeError(
                        "Fused and separated preparation changed stored data."
                    )
                report["cases"].append(
                    {
                        "name": name,
                        "limitsBeforeStart": limits,
                        "job": job,
                        "measurement": value,
                    }
                )
                print(
                    json.dumps(
                        {
                            "case": name,
                            "records": RECORDS,
                            "cpuMs": {
                                key: value[key]["cpuMs"]
                                for key in [
                                    "decode",
                                    "raw",
                                    "entry",
                                    "fusedRawEntry",
                                    "insert",
                                    "publishSwap",
                                ]
                            },
                        }
                    ),
                    flush=True,
                )
        report["passed"] = len(report["cases"]) == len(CASES)
    except BaseException as error:
        report["failureType"] = type(error).__name__
        raise
    finally:
        finished.set()
        for process in active:
            stop_tree(process)
        report["elapsedSeconds"] = time.monotonic() - started
        if "inputs" in report:
            report["inputsUnchanged"] = all(
                sha(ROOT / name) == digest for name, digest in report["inputs"].items()
            )
            report["passed"] = report["passed"] and report["inputsUnchanged"]
        if "testExecutableSha256" in report:
            report["testExecutableUnchanged"] = (
                sha(executable) == report["testExecutableSha256"]
            )
            report["passed"] = report["passed"] and report["testExecutableUnchanged"]
        report["passed"] = (
            report["passed"] and not report["resourceStop"] and not report["timeStop"]
        )
        (output / "result.json").write_text(
            json.dumps(report, indent=2), encoding="utf8"
        )
    if not report["passed"]:
        raise RuntimeError("CPU qualification did not complete with stable inputs.")


if __name__ == "__main__":
    main()
