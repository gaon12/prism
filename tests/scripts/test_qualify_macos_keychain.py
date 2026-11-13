import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "keychain_qualification", ROOT / "scripts/qualify-macos-keychain.py"
)
QA = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(QA)


class FakeSecurity:
    """Model preference mutations only; never exercise an OS credential store."""

    def __init__(self):
        self.original = {
            "default": "/user/login.keychain-db",
            "search": [
                "/user/login.keychain-db",
                "/user/Other keychain.keychain-db",
            ],
        }
        self.default = self.original["default"]
        self.search = list(self.original["search"])
        self.calls = []
        self.partial_create_failure = False
        self.restore_default_failure = False
        self.reject_isolation = False

    @contextmanager
    def isolated(self, parent, report):
        # Native preference reads are modeled separately from the macOS output
        # parser, so these restoration checks also run on Windows hosts.
        def preferences(_security):
            return {"default": self.default, "search": list(self.search)}

        with (
            patch.object(QA, "snapshot", side_effect=preferences),
            QA.isolated_keychain(self, parent, report),
        ):
            yield

    def __call__(self, *args):
        self.calls.append(args)
        name = args[0]
        if name == "create-keychain":
            Path(args[-1]).touch()
            self.search.append(args[-1])
            if self.partial_create_failure:
                raise QA.QualificationError("Synthetic partial creation failure")
        elif name == "delete-keychain":
            Path(args[-1]).unlink()
            self.search = [path for path in self.search if path != args[-1]]
        elif name == "default-keychain":
            if "-s" in args:
                if (
                    self.restore_default_failure
                    and args[-1] == self.original["default"]
                ):
                    raise QA.QualificationError("Synthetic restore failure")
                self.default = args[-1]
            else:
                return f'    "{self.default}"\n'
        elif name == "list-keychains":
            if "-s" in args:
                if (
                    not self.reject_isolation
                    or list(args[4:]) == self.original["search"]
                ):
                    self.search = list(args[4:])
            else:
                return "".join(f'    "{path}"\n' for path in self.search)
        elif name not in ["unlock-keychain", "set-keychain-settings"]:
            raise AssertionError(name)
        return ""


class QualificationTests(unittest.TestCase):
    def test_non_macos_main_refuses_before_any_output_or_child(self):
        with tempfile.TemporaryDirectory() as parent:
            output = Path(parent) / "must-not-exist"
            with (
                patch.object(sys, "argv", ["qualify", "--output", str(output)]),
                patch.object(QA.platform, "system", return_value="Windows"),
                patch.object(QA, "Security") as security,
                patch.object(QA, "bounded_run") as child,
                self.assertRaises(QA.QualificationError),
            ):
                QA.main()
            self.assertFalse(output.exists())
            security.assert_not_called()
            child.assert_not_called()

    def test_snapshot_reads_only_user_preferences_and_rejects_missing_default(self):
        security = FakeSecurity()
        self.assertEqual(QA.snapshot(security), security.original)
        self.assertEqual(
            security.calls,
            [
                ("default-keychain", "-d", "user"),
                ("list-keychains", "-d", "user"),
            ],
        )
        with self.assertRaises(QA.QualificationError):
            QA.snapshot(lambda *_args: "")

    def test_host_guard_requires_all_hosted_macos_conditions(self):
        env = {
            "GITHUB_ACTIONS": "true",
            "RUNNER_ENVIRONMENT": "github-hosted",
            "RUNNER_OS": "macOS",
        }
        QA.require_host("Darwin", env)
        for system, changed in [("Windows", env), ("Linux", env)] + [
            ("Darwin", {**env, key: value})
            for key, value in [
                ("GITHUB_ACTIONS", "false"),
                ("RUNNER_ENVIRONMENT", "self-hosted"),
                ("RUNNER_OS", "Windows"),
            ]
        ]:
            with (
                self.subTest(system=system, env=changed),
                self.assertRaises(QA.QualificationError),
            ):
                QA.require_host(system, changed)

    def test_security_paths_preserve_spaces_quotes_and_backslashes_as_data(self):
        value = '/user/a "quoted" \\ $name.keychain-db'
        self.assertEqual(QA.keychain_paths(f'    "{value}"\n'), [value])
        for invalid in [
            "relative.keychain",
            '"relative.keychain"',
            "<NULL>",
            '"/a\nb"',
            '"/a\0b"',
        ]:
            with (
                self.subTest(invalid=invalid),
                self.assertRaises(QA.QualificationError),
            ):
                QA.keychain_paths(invalid)

    def test_success_isolates_then_restores_and_deletes_only_owned_keychain(self):
        security, report = FakeSecurity(), {}
        with tempfile.TemporaryDirectory() as parent:
            with security.isolated(parent, report):
                self.assertTrue(report["isolationVerified"])
                self.assertEqual(security.search, [security.default])
                self.assertNotEqual(security.default, security.original["default"])
            self.assertEqual(QA.snapshot(security), security.original)
            self.assertTrue(report["restorationVerified"])
            self.assertTrue(report["temporaryKeychainRemoved"])
            deleted = [
                args[1] for args in security.calls if args[0] == "delete-keychain"
            ]
            self.assertEqual(deleted, [report["temporaryKeychain"]])
            password = next(
                args[2] for args in security.calls if args[0] == "create-keychain"
            )
            self.assertNotIn(password, json.dumps(report))

    def test_failed_test_still_restores_configuration(self):
        security, report = FakeSecurity(), {}
        with tempfile.TemporaryDirectory() as parent:
            with (
                self.assertRaisesRegex(ValueError, "synthetic test failure"),
                security.isolated(parent, report),
            ):
                raise ValueError("synthetic test failure")
            self.assertEqual(QA.snapshot(security), security.original)
            self.assertTrue(report["temporaryKeychainRemoved"])

    def test_partial_creation_failure_restores_creation_side_effect(self):
        security, report = FakeSecurity(), {}
        security.partial_create_failure = True
        with tempfile.TemporaryDirectory() as parent:
            with (
                self.assertRaises(QA.QualificationError),
                security.isolated(parent, report),
            ):
                self.fail("Test must not run after partial keychain creation")
            self.assertEqual(QA.snapshot(security), security.original)
            self.assertTrue(report["temporaryKeychainRemoved"])

    def test_unverified_isolation_never_runs_test(self):
        security, report = FakeSecurity(), {}
        security.reject_isolation = True
        with tempfile.TemporaryDirectory() as parent:
            with (
                self.assertRaisesRegex(QA.QualificationError, "isolation"),
                security.isolated(parent, report),
            ):
                self.fail("Test must not run without verified isolation")
            self.assertEqual(QA.snapshot(security), security.original)

    def test_restore_failure_still_restores_search_and_retains_selected_keychain(self):
        security, report = FakeSecurity(), {}
        security.restore_default_failure = True
        with tempfile.TemporaryDirectory() as parent:
            with (
                self.assertRaisesRegex(QA.QualificationError, "not restored"),
                security.isolated(parent, report),
            ):
                pass
            self.assertEqual(security.search, security.original["search"])
            self.assertFalse(report["restorationVerified"])
            self.assertTrue(Path(report["temporaryKeychain"]).is_file())
            self.assertFalse(
                any(args[0] == "delete-keychain" for args in security.calls)
            )

    def test_security_failure_does_not_include_password_arguments(self):
        password = "synthetic-private-keychain-password"
        failure = subprocess.TimeoutExpired(["security", "-p", password], 20)
        with (
            patch.object(QA.subprocess, "run", side_effect=failure),
            self.assertRaises(QA.QualificationError) as raised,
        ):
            QA.Security()("create-keychain", "-p", password, "/tmp/test.keychain-db")
        self.assertNotIn(password, str(raised.exception))

    def test_build_artifact_selection_requires_exact_test_target(self):
        with tempfile.TemporaryDirectory() as parent:
            directory = Path(parent)
            executable = directory / "test binary"
            executable.touch()
            log = directory / "build.jsonl"
            item = {
                "reason": "compiler-artifact",
                "profile": {"test": True},
                "target": {"name": "prism_rag_qa"},
                "executable": str(executable),
            }
            log.write_text(json.dumps(item), encoding="utf8")
            self.assertEqual(QA.test_executable(log), executable.resolve())
            log.write_text(
                json.dumps({**item, "target": {"name": "other"}}), encoding="utf8"
            )
            with self.assertRaises(QA.QualificationError):
                QA.test_executable(log)
            log.write_text(json.dumps(item) + "\n" + json.dumps(item), encoding="utf8")
            with self.assertRaises(QA.QualificationError):
                QA.test_executable(log)


if __name__ == "__main__":
    unittest.main()
