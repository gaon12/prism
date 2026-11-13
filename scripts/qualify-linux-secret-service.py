"""Exercise production credentials on a new private Linux bus and keyring only."""
import argparse
import hashlib
import json
import os
import re
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST = "secret_store::native::tests::native_store_roundtrip_replacement_isolation_and_delete"
GIB = 1024**3


class QualificationError(RuntimeError):
    pass


def available_memory():
    return int(re.search(r"^MemAvailable:\s+(\d+)", Path("/proc/meminfo").read_text(), re.M)[1]) * 1024


def terminate(process):
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            process.wait(timeout=5)
            return
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def qualify(args):
    if sys.platform != "linux":
        raise QualificationError("Requires Linux and a disposable local Secret Service daemon.")
    if not args.daemon or not Path(args.daemon).is_file():
        raise QualificationError("An explicit available GNOME Keyring daemon is required.")
    args.daemon = str(Path(args.daemon).resolve(strict=True))
    if not all(shutil.which(tool) for tool in ["cargo", "dbus-daemon", "gdbus"]):
        raise QualificationError("Cargo, dbus-daemon and gdbus are required.")
    target = Path(os.environ.get("CARGO_TARGET_DIR", ""))
    if not target.is_absolute() or target.is_symlink():
        raise QualificationError("Set an absolute scratch CARGO_TARGET_DIR.")
    if available_memory() < 3 * GIB:
        raise QualificationError("Requires 3 GiB available memory before compilation.")
    output = Path(args.output).absolute()
    if output.exists() or output.is_symlink() or not output.parent.is_dir():
        raise QualificationError("The evidence directory must be new, with an existing parent.")
    if shutil.disk_usage(output.parent).free < 2 * GIB:
        raise QualificationError("Requires 2 GiB free evidence disk space.")
    output.mkdir(mode=0o700)
    inputs = ["src-tauri/src/secret_store.rs", "src-tauri/src/native_secrets.rs",
              "tools/rag-qa/Cargo.toml", "tools/rag-qa/Cargo.lock", "tools/rag-qa/src/lib.rs",
              "tools/rag-qa/src/bin/secret-store-qa.rs", "scripts/qualify-linux-secret-service.py"]
    before = {path: digest(ROOT / path) for path in inputs}
    report = {"passed": False, "scope": "Production credential API on a private Linux bus/keyring, not desktop UI or Flatpak.",
              "sourceCommit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "inputs": before, "minimumAvailable": available_memory(), "phases": [], "cleanupVerified": False}
    report["inputsMatchHead"] = all(
        subprocess.run(["git", "show", "HEAD:" + path], cwd=ROOT, capture_output=True).stdout == (ROOT / path).read_bytes()
        for path in inputs
    )
    env = os.environ.copy()
    env.update(CARGO_BUILD_JOBS="1", CARGO_NET_OFFLINE="true")
    processes = []
    private = None
    started = time.monotonic()

    def guard():
        report["minimumAvailable"] = min(report["minimumAvailable"], available_memory())
        if report["minimumAvailable"] < GIB:
            raise QualificationError("Available memory fell below 1 GiB.")
        if args.stop_file and Path(args.stop_file).exists():
            raise QualificationError("External resource guard requested a stop.")
        if shutil.disk_usage(output).free < 2 * GIB:
            raise QualificationError("Evidence disk space fell below 2 GiB.")

    def command(argv, label, seconds=20, input_data=None):
        guard()
        with (output / (label + ".stdout.log")).open("wb") as stdout, (output / (label + ".stderr.log")).open("wb") as stderr:
            process = subprocess.Popen(argv, cwd=ROOT, env=env, stdin=subprocess.PIPE if input_data else subprocess.DEVNULL,
                                       stdout=stdout, stderr=stderr, start_new_session=True)
            processes.append(process)
            try:
                if input_data:
                    process.stdin.write(input_data)
                    process.stdin.close()
                deadline = time.monotonic() + seconds
                while process.poll() is None:
                    guard()
                    if time.monotonic() > deadline:
                        raise QualificationError(f"{label} exceeded its bounded deadline.")
                    time.sleep(0.2)
                if process.returncode:
                    raise QualificationError(f"{label} failed ({process.returncode}); preserved logs describe the failure.")
            finally:
                terminate(process)
        return (output / (label + ".stdout.log")).read_text()

    def bus_call(destination, object_path, method, *arguments):
        guard()
        try:
            result = subprocess.run(["gdbus", "call", "--session", "--dest", destination,
                                     "--object-path", object_path, "--method", method, *arguments],
                                    env=env, capture_output=True, text=True, timeout=5, check=False)
        except subprocess.TimeoutExpired:
            raise QualificationError("Private bus call timed out.") from None
        if result.returncode:
            raise QualificationError("Private bus call failed.")
        return result.stdout.strip()

    try:
        binaries = {}
        for label, mode in [("build-test", ["test", "--lib", "--no-run"]), ("build-probe", ["build", "--bin", "secret-store-qa"])]:
            text = command(["cargo", *mode, "--manifest-path", "tools/rag-qa/Cargo.toml", "--locked", "--offline", "-j1", "--message-format=json"], label, 900)
            artifacts = [json.loads(line) for line in text.splitlines() if line.startswith("{")]
            executable = [item["executable"] for item in artifacts if item.get("reason") == "compiler-artifact" and item.get("executable")]
            if len(executable) != 1:
                raise QualificationError("Expected exactly one compiled credential binary.")
            binaries[label] = Path(executable[0])
        report["executables"] = {key: {"path": str(path), "sha256": digest(path)} for key, path in binaries.items()}
        private = Path(tempfile.mkdtemp(prefix="prism-secret-service-"))
        nonce = secrets.token_hex(16)
        (private / "nonce").write_text(nonce)
        os.chmod(private / "nonce", 0o600)
        for name in ["runtime", "data", "config", "cache", "control"]:
            (private / name).mkdir(mode=0o700)
        for key in ["DBUS_SESSION_BUS_ADDRESS", "GNOME_KEYRING_CONTROL", "GNOME_KEYRING_PID", "DISPLAY", "WAYLAND_DISPLAY"]:
            env.pop(key, None)
        env.update({"XDG_" + key + "_HOME": str(private / value) for key, value in [("DATA", "data"), ("CONFIG", "config"), ("CACHE", "cache")]})
        address = "unix:path=" + str(private / "runtime/bus")
        env.update(DBUS_SESSION_BUS_ADDRESS=address, XDG_RUNTIME_DIR=str(private / "runtime"),
                   PRISM_TEST_OS_CREDENTIALS="1", PRISM_TEST_SECRET_RUNTIME=str(private / "runtime"))
        if args.library_path:
            env["LD_LIBRARY_PATH"] = str(Path(args.library_path).resolve())
        # HOME and the login-session bus/keyring are never redirected or selected.
        with (output / "bus.log").open("wb") as bus_log, (output / "daemon.log").open("wb") as daemon_log:
            bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--nopidfile", "--address=" + address], env=env,
                                   stdin=subprocess.DEVNULL, stdout=bus_log, stderr=bus_log, start_new_session=True)
            processes.append(bus)
            report["privateBusPid"] = bus.pid
            deadline = time.monotonic() + 10
            while not (private / "runtime/bus").exists():
                guard()
                if bus.poll() is not None or time.monotonic() > deadline:
                    raise QualificationError("Private bus did not start.")
                time.sleep(0.1)

            def peer(name):
                reply = bus_call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.GetConnectionUnixProcessID", name)
                match = re.fullmatch(r"\(uint32 (\d+),\)", reply)
                if not match:
                    raise QualificationError("Unexpected private-bus peer identity.")
                return int(match[1])

            if peer("org.freedesktop.DBus") != bus.pid:
                raise QualificationError("Bus peer did not match the owned daemon.")
            password = secrets.token_hex(32).encode()
            report["daemonExecutableSha256"] = digest(Path(args.daemon))
            daemon = subprocess.Popen([args.daemon, "--foreground", "--unlock", "--components=secrets", "--control-directory=" + str(private / "control")],
                                      env=env, stdin=subprocess.PIPE, stdout=daemon_log, stderr=daemon_log, start_new_session=True)
            processes.append(daemon)
            report["initialDaemonPid"] = daemon.pid
            daemon.stdin.write(password)
            daemon.stdin.close()
            command(["gdbus", "wait", "--session", "--timeout", "10", "org.freedesktop.secrets"], "wait-service", 12)

            def isolation():
                if bus.poll() is not None or daemon.poll() is not None or peer("org.freedesktop.DBus") != bus.pid:
                    raise QualificationError("Owned private session exited.")
                owner = peer("org.freedesktop.secrets")
                if os.getpgid(owner) != daemon.pid:
                    raise QualificationError("Secret Service was not owned by the private daemon group.")
                report["isolationVerified"] = True

            def phase(name):
                isolation()
                text = command([str(binaries["build-probe"]), name, nonce], name)
                if json.loads(text) != {"passed": True, "phase": name}:
                    raise QualificationError("Unexpected credential phase result.")
                report["phases"].append(name)

            isolation()
            native = command([str(binaries["build-test"]), TEST, "--exact", "--ignored", "--nocapture"], "native-roundtrip", 60)
            if "1 passed; 0 failed; 0 ignored" not in native:
                raise QualificationError("The exact native credential test did not pass once.")
            phase("prepare")
            alias = bus_call("org.freedesktop.secrets", "/org/freedesktop/secrets", "org.freedesktop.Secret.Service.ReadAlias", "default")
            match = re.fullmatch(r"\(objectpath '(/org/freedesktop/secrets/collection/[A-Za-z0-9_]+)',\)", alias)
            if not match:
                raise QualificationError("Private default collection was not available.")
            collection = match[1]
            report["privateCollection"] = collection
            bus_call("org.freedesktop.secrets", "/org/freedesktop/secrets", "org.freedesktop.Secret.Service.Lock", "['" + collection + "']")
            locked = bus_call("org.freedesktop.secrets", collection, "org.freedesktop.DBus.Properties.Get", "org.freedesktop.Secret.Collection", "Locked")
            if locked != "(<true>,)":
                raise QualificationError("Private collection did not lock.")
            phase("unavailable")
            if (private / "unused-profile").exists():
                raise QualificationError("A profile fallback was created while storage was locked.")
            report["noProfileFallbackCreated"] = True
            isolation()
            # --unlock supplies a startup password; it is not a portable remote
            # control operation for an existing GNOME daemon. Restart only our
            # foreground daemon, retaining its private on-disk keyring, then
            # prove that the failed operations did not replace/delete the key.
            terminate(daemon)
            daemon = subprocess.Popen([args.daemon, "--foreground", "--unlock", "--components=secrets", "--control-directory=" + str(private / "control")],
                                      env=env, stdin=subprocess.PIPE, stdout=daemon_log, stderr=daemon_log, start_new_session=True)
            processes.append(daemon)
            report["restartedDaemonPid"] = daemon.pid
            daemon.stdin.write(password)
            daemon.stdin.close()
            command(["gdbus", "wait", "--session", "--timeout", "10", "org.freedesktop.secrets"], "wait-restarted-service", 12)
            isolation()
            password = b""
            locked = bus_call("org.freedesktop.secrets", collection, "org.freedesktop.DBus.Properties.Get", "org.freedesktop.Secret.Collection", "Locked")
            if locked != "(<false>,)":
                raise QualificationError("Private collection did not unlock.")
            phase("verify")
            phase("remove")
            report["lockedFailurePreservedCredential"] = True
        report["inputsUnchanged"] = before == {path: digest(ROOT / path) for path in inputs}
        if not report["inputsUnchanged"]:
            raise QualificationError("Qualification inputs changed during execution.")
        report["executablesUnchanged"] = all(
            digest(path) == report["executables"][key]["sha256"]
            for key, path in binaries.items()
        ) and digest(Path(args.daemon)) == report["daemonExecutableSha256"]
        if not report["executablesUnchanged"]:
            raise QualificationError("Qualification executables changed during execution.")
        report["passed"] = True
    except (QualificationError, OSError) as error:
        report["error"] = str(error)
    finally:
        cleanup_errors = []
        for process in reversed(processes):
            try:
                terminate(process)
            except (OSError, subprocess.TimeoutExpired):
                cleanup_errors.append("An owned process could not be reaped.")
        if private is not None:
            if private.is_symlink() or private.resolve().parent != Path(tempfile.gettempdir()).resolve() or not private.name.startswith("prism-secret-service-"):
                cleanup_errors.append("Private-directory ownership check failed; directory retained.")
            else:
                try:
                    shutil.rmtree(private)
                except OSError:
                    cleanup_errors.append("The private keyring directory could not be removed.")
        report["cleanupVerified"] = (
            not cleanup_errors
            and (private is None or not private.exists())
            and all(process.poll() is not None for process in processes)
        )
        if cleanup_errors:
            report["cleanupErrors"] = cleanup_errors
        report["passed"] = report["passed"] and report["cleanupVerified"]
        report["elapsedSeconds"] = time.monotonic() - started
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "evidence": str(output), "cleanupVerified": report["cleanupVerified"]}))
    return 0 if report["passed"] and report["cleanupVerified"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--daemon", default=shutil.which("gnome-keyring-daemon"))
    parser.add_argument("--library-path")
    parser.add_argument("--stop-file", help="Optional external-resource interlock; its presence stops owned work.")
    args = parser.parse_args()
    try:
        return qualify(args)
    except QualificationError as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
