"""Own one Unix Office engine group until its worker's private pipe closes.

No document bytes or user-selected executable are accepted by this helper.
The supervisor deliberately keeps its Child unreaped until group cleanup, so
the engine leader's PID/group cannot be reused between signalling and wait.
"""

import ctypes
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

OWNER = b"Prism Office preview worker v1\n"
READY = b"PRISMOFFICE1\n"


def private_paths(runtime, profile, pipe):
    runtime, profile = Path(runtime), Path(profile)
    if not runtime.is_absolute() or not profile.is_absolute():
        raise ValueError("paths")
    for path in [runtime, profile]:
        if any(part.is_symlink() for part in [path, *path.parents]):
            raise ValueError("link")
    if (profile / "owner.txt").read_bytes() != OWNER:
        raise ValueError("owner")
    token = pipe.removeprefix("prism-office-")
    if len(token) != 32 or any(value not in "0123456789abcdef" for value in token):
        raise ValueError("pipe")
    return runtime, profile


def command(runtime, profile, pipe):
    return [
        str(runtime / "program/soffice.bin"),
        "--headless",
        "--nologo",
        "--nodefault",
        "--nofirststartwizard",
        "--norestore",
        "-env:CrashDumpEnable=false",
        "-env:UserInstallation=" + (profile / "user").as_uri(),
        "--accept=pipe,name=" + pipe + ";urp;StarOffice.ComponentContext",
    ]


def bounded_engine(runtime, profile, pipe):
    import resource

    # Apply limits in this single-threaded exec helper, not a preexec_fn in the
    # multi-threaded supervisor. Existing lower inherited limits stay lower.
    for kind, bound in [
        (resource.RLIMIT_CORE, 0),
        (resource.RLIMIT_AS, 2 << 30),
        (resource.RLIMIT_FSIZE, 64 << 20),
        (resource.RLIMIT_CPU, 90),
        (resource.RLIMIT_NOFILE, 256),
    ]:
        soft, hard = resource.getrlimit(kind)
        soft = bound if soft == resource.RLIM_INFINITY else min(soft, bound)
        hard = bound if hard == resource.RLIM_INFINITY else min(hard, bound)
        resource.setrlimit(kind, (min(soft, hard), hard))
    args = command(runtime, profile, pipe)
    os.execve(args[0], args, os.environ)


def supervise(runtime, profile, pipe):
    # An inherited SIG_IGN would let the kernel auto-reap a child and invalidate
    # the reserved-PID guarantee used for group signalling below.
    signal.signal(signal.SIGCHLD, signal.SIG_DFL)
    # Adopt orphaned engine grandchildren into this private supervisor so they
    # can be reaped here instead of depending on a desktop/container's PID 1.
    libc = ctypes.CDLL(None, use_errno=True)
    libc.prctl.argtypes = [
        ctypes.c_int,
        ctypes.c_ulong,
        ctypes.c_ulong,
        ctypes.c_ulong,
        ctypes.c_ulong,
    ]
    libc.prctl.restype = ctypes.c_int
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "subreaper")
    closed = threading.Event()

    def parent():
        try:
            os.read(0, 1)
        finally:
            closed.set()

    threading.Thread(target=parent, daemon=True).start()
    if closed.is_set():
        return
    for attempt in range(3):
        if closed.is_set():
            return
        engine = subprocess.Popen(
            [
                sys.executable,
                "-P",
                "-S",
                "-B",
                str(Path(__file__).resolve()),
                "--engine",
                str(runtime),
                str(profile),
                pipe,
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            process_group=0,
            close_fds=True,
        )
        try:
            if attempt == 0:
                sys.stdout.buffer.write(READY)
                sys.stdout.buffer.flush()
            while not closed.wait(0.05):
                # Observe exit without reaping: group cleanup still happens
                # while the leader's PID is reserved by this exact Child.
                exited = os.waitid(
                    os.P_PID, engine.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT
                )
                if exited is not None:
                    break
        finally:
            try:
                os.killpg(engine.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            status = engine.wait(timeout=5)
            deadline = time.monotonic() + 2
            while True:
                try:
                    adopted, _ = os.waitpid(-1, os.WNOHANG)
                except ChildProcessError:
                    break
                if adopted == 0:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("reap")
                    time.sleep(0.01)
        if closed.is_set() or status != 81:
            return
        # Direct soffice.bin requests a restart after initial profile setup.
        # Bound the retries; never fall back to an installed external engine.
    raise RuntimeError("restart")


class UnixEngine:
    def __init__(self, runtime, profile, pipe, environment):
        private_paths(runtime, profile, pipe)
        self.process = subprocess.Popen(
            [
                sys.executable,
                "-P",
                "-S",
                "-B",
                str(Path(__file__).resolve()),
                "--supervise",
                str(runtime),
                str(profile),
                pipe,
            ],
            env=environment,
            cwd=profile,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
            close_fds=True,
        )
        self.lock = threading.Lock()
        if self.process.stdout.read(len(READY)) != READY:
            self.kill()
            self.process.wait(timeout=5)
            raise RuntimeError("supervisor")

    def poll(self):
        return self.process.poll()

    def kill(self):
        # Closing the worker's ONLY writer handles normal completion, explicit
        # cancellation and kernel descriptor closure after a worker SIGKILL.
        with self.lock:
            if self.process.stdin is not None:
                self.process.stdin.close()
                self.process.stdin = None

    def wait(self, timeout=None):
        self.kill()
        result = self.process.wait(timeout=timeout)
        self.process.stdout.close()
        if result != 0:
            raise RuntimeError("supervisor")
        return result


def main():
    if sys.platform != "linux" or len(sys.argv) != 5:
        raise ValueError("arguments")
    role, runtime, profile, pipe = sys.argv[1:]
    runtime, profile = private_paths(runtime, profile, pipe)
    if role == "--supervise":
        supervise(runtime, profile, pipe)
    elif role == "--engine":
        bounded_engine(runtime, profile, pipe)
    else:
        raise ValueError("role")


if __name__ == "__main__":
    try:
        main()
    except Exception:  # noqa: BLE001 -- fixed exit code, no native details
        raise SystemExit(70) from None
