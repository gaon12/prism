"""Linux process-lifetime fixtures, separate from real Office rendering tests."""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("unix_owner", ROOT / "unix_owner.py")
owner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owner)


def identity(pid):
    try:
        parts = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return parts[19], parts[0]
    except FileNotFoundError:
        return None


def ended(pid, birth):
    now = identity(pid)
    return now is None or now[0] != birth or now[1] == "Z"


@unittest.skipUnless(sys.platform == "linux", "Linux-owned process groups")
class UnixOwner(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="prism-unix-owner-")
        self.root = Path(self.directory.name)
        self.runtime = self.root / "runtime"
        self.profile = self.root / "work"
        (self.runtime / "program").mkdir(parents=True)
        self.profile.mkdir()
        (self.profile / "owner.txt").write_bytes(owner.LEASED_OWNER)
        self.pipe = "prism-office-" + uuid.uuid4().hex
        self.canary = subprocess.Popen(
            [sys.executable, "-B", "-c", "import time; time.sleep(120)"]
        )

    def tearDown(self):
        self.canary.terminate()
        self.canary.wait(timeout=5)
        self.directory.cleanup()

    def fixture(self, exits=False):
        # An inert owned executable exercises descendants and early leader exit.
        # It is never used as a renderer or model in any actual document test.
        script = self.runtime / "program/soffice.bin"
        script.write_text(
            f"""#!{sys.executable}
import json,subprocess,sys,time,signal,resource
from pathlib import Path
child=subprocess.Popen([sys.executable,"-B","-c","import time; time.sleep(120)"])
def birth(pid): return Path(f"/proc/{{pid}}/stat").read_text().rsplit(")",1)[1].split()[19]
pid=__import__("os").getpid()
(Path(__file__).parent.parent/"limits.json").write_text(json.dumps({{"sigchld":int(signal.getsignal(signal.SIGCHLD)),"core":resource.getrlimit(resource.RLIMIT_CORE)[0],"as":resource.getrlimit(resource.RLIMIT_AS)[0],"files":resource.getrlimit(resource.RLIMIT_NOFILE)[0]}}))
(Path(__file__).parent.parent/"pids.json").write_text(json.dumps([[pid,birth(pid)],[child.pid,birth(child.pid)]]))
{"sys.exit(0)" if exits else "time.sleep(120)"}
""",
            encoding="utf-8",
        )
        script.chmod(0o755)

    def pids(self):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                return json.loads((self.runtime / "pids.json").read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                time.sleep(0.01)
        self.fail("Fixture engine did not start")

    def assert_ended(self, pids):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not all(
            ended(pid, birth) for pid, birth in pids
        ):
            time.sleep(0.01)
        self.assertTrue(all(ended(pid, birth) for pid, birth in pids))
        self.assertIsNone(self.canary.poll(), "An unrelated owned canary must survive")

    def test_normal_stop_is_idempotent_and_reaps_engine_grandchild(self):
        self.fixture()
        engine = owner.UnixEngine(
            self.runtime, self.profile, self.pipe, dict(os.environ)
        )
        try:
            pids = self.pids()
            import fcntl

            with (
                (self.profile / "owner.txt").open("rb") as lease,
                self.assertRaises(BlockingIOError),
            ):
                fcntl.flock(lease.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            self.assertEqual(engine.wait(timeout=5), 0)
        self.assertEqual(engine.wait(timeout=5), 0)
        self.assert_ended(pids)
        self.assertTrue(all(identity(pid) is None for pid, _ in pids))
        with (self.profile / "owner.txt").open("rb") as lease:
            fcntl.flock(lease.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        limits = json.loads((self.runtime / "limits.json").read_text())
        self.assertEqual(limits["sigchld"], 0)
        self.assertEqual(limits["core"], 0)
        self.assertLessEqual(limits["as"], 2 << 30)
        self.assertLessEqual(limits["files"], 256)

    def test_exited_leader_does_not_hide_its_live_grandchild(self):
        self.fixture(exits=True)
        engine = owner.UnixEngine(
            self.runtime, self.profile, self.pipe, dict(os.environ)
        )
        pids = self.pids()
        self.assertEqual(engine.process.wait(timeout=5), 0)
        self.assertEqual(engine.wait(timeout=5), 0)
        self.assert_ended(pids)
        self.assertTrue(all(identity(pid) is None for pid, _ in pids))

    def test_worker_sigkill_closes_its_only_supervisor_writer(self):
        self.fixture()
        code = """import importlib.util,json,os,sys,time
from pathlib import Path
spec=importlib.util.spec_from_file_location("owner",sys.argv[1]); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
engine=module.UnixEngine(Path(sys.argv[2]),Path(sys.argv[3]),sys.argv[4],dict(os.environ))
pid=engine.process.pid
birth=Path(f"/proc/{pid}/stat").read_text().rsplit(")",1)[1].split()[19]
print(json.dumps([pid,birth]),flush=True)
time.sleep(120)
"""
        worker = subprocess.Popen(
            [
                sys.executable,
                "-B",
                "-c",
                code,
                str(ROOT / "unix_owner.py"),
                str(self.runtime),
                str(self.profile),
                self.pipe,
            ],
            stdout=subprocess.PIPE,
        )
        try:
            supervisor = json.loads(worker.stdout.readline())
            pids = self.pids()
        finally:
            worker.kill()
            worker.wait(timeout=5)
            worker.stdout.close()
        self.assert_ended([*pids, supervisor])

    def test_bad_profile_or_pipe_cannot_launch_an_executable(self):
        with self.assertRaises(ValueError):
            owner.private_paths(self.runtime, self.profile, "prism-office-x;bad")
        (self.profile / "owner.txt").write_bytes(b"not owned")
        with self.assertRaises(ValueError):
            owner.private_paths(self.runtime, self.profile, self.pipe)

    def test_ignored_sigchld_in_the_worker_is_reset_before_creating_an_engine(self):
        self.fixture(exits=True)
        code = """import importlib.util,os,signal,sys,time
from pathlib import Path
signal.signal(signal.SIGCHLD,signal.SIG_IGN)
spec=importlib.util.spec_from_file_location("owner",sys.argv[1]); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
engine=module.UnixEngine(Path(sys.argv[2]),Path(sys.argv[3]),sys.argv[4],dict(os.environ))
deadline=time.monotonic()+5
while not (Path(sys.argv[2])/"pids.json").exists():
 if time.monotonic()>deadline: raise RuntimeError("fixture start")
 time.sleep(0.01)
engine.wait(timeout=5)
"""
        subprocess.run(
            [
                sys.executable,
                "-B",
                "-c",
                code,
                str(ROOT / "unix_owner.py"),
                str(self.runtime),
                str(self.profile),
                self.pipe,
            ],
            check=True,
            timeout=10,
        )
        self.assert_ended(self.pids())
        self.assertEqual(
            json.loads((self.runtime / "limits.json").read_text())["sigchld"], 0
        )


if __name__ == "__main__":
    unittest.main()
