"""Opt-in real PyUNO bootstrap check, never using a normal user profile.

The broken extension registry is an owned negative fixture. A control child
must actually reject it; the production bootstrap must ignore that profile
and read only its own private configuration. No Office/UNO API is replaced.
"""

import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def probe():
    runtime, profile = map(Path, sys.argv[2:4])
    mode = sys.argv[4]
    sys.dont_write_bytecode = True
    os.environ["URE_BOOTSTRAP"] = (runtime / "program/fundamental.ini").as_uri()
    os.environ["UNO_PATH"] = str(runtime / "program")
    if mode == "private":
        # Import the exact production helper without running a conversion.
        sys.path.insert(0, str(ROOT))
        import worker

        expected = worker.configure_bootstrap(runtime, profile)
    else:
        expected = (profile / "user").as_uri()
    dll_directory = os.add_dll_directory(str(runtime / "program"))
    sys.path.insert(0, str(runtime / "program"))
    try:
        import uno

        context = uno.getComponentContext()
    except SystemError as issue:
        # Only identify the deliberately invalid fixture, never print a native
        # exception that could contain an arbitrary profile or document path.
        print(
            json.dumps(
                {"bootstrapOK": False, "canaryError": "bad-services.rdb" in str(issue)}
            )
        )
        return
    if mode == "control":
        # If an upstream engine stops rejecting the negative registry, fail
        # without instantiating its host-registry-backed configuration provider.
        print(json.dumps({"bootstrapOK": True, "unexpectedControlSuccess": True}))
        return
    macro = context.getValueByName("/singletons/com.sun.star.util.theMacroExpander")
    user = macro.expandMacros(
        "${$BRAND_BASE_DIR/program/bootstrap.ini:UserInstallation}"
    )
    packages = macro.expandMacros("$UNO_USER_PACKAGES_CACHE")
    layers = macro.expandMacros("$CONFIGURATION_LAYERS")
    provider = context.ServiceManager.createInstanceWithContext(
        "com.sun.star.configuration.ConfigurationProvider", context
    )
    node = uno.createUnoStruct("com.sun.star.beans.PropertyValue")
    node.Name, node.Value = "nodepath", "/org.openoffice.Setup/L10N"
    access = provider.createInstanceWithArguments(
        "com.sun.star.configuration.ConfigurationAccess", (node,)
    )
    print(
        json.dumps(
            {
                "bootstrapOK": True,
                "privateUser": user == expected,
                "privatePackages": packages.startswith(expected + "/"),
                "hostRegistryLayer": "winreg:" in layers,
                "canaryConfiguration": os.environ["SYSUSERCONFIG"] in layers,
                "privateLocale": access.getPropertyValue("ooLocale") == "ko-KR",
            }
        )
    )
    # Keep the DLL search directory alive until all native objects are finished.
    assert dll_directory is not None


def main():
    assert sys.platform == "win32"
    if len(sys.argv) > 1 and sys.argv[1] == "--probe":
        probe()
        return
    runtime = Path(os.environ["PRISM_OFFICE_QA_RUNTIME"]).resolve(strict=True)
    catalog = json.loads((ROOT / "windows-x64.lock.json").read_text(encoding="utf-8"))
    assert digest(runtime / "inventory.json") == catalog["inventorySha256"]
    inventory = json.loads((runtime / "inventory.json").read_text(encoding="utf-8"))
    assert len(inventory) == catalog["fileCount"]
    expected_files = {item["path"] for item in inventory} | {
        "inventory.json",
        ".prism-office-runtime",
    }
    actual_files = {
        file.relative_to(runtime).as_posix()
        for file in runtime.rglob("*")
        if file.is_file()
    }
    assert actual_files == expected_files, "Unexpected or missing engine files"
    for item in inventory:
        file = runtime / item["path"]
        assert file.is_file() and not file.is_symlink()
        assert file.stat().st_size == item["bytes"] and digest(file) == item["sha256"]
    owned = Path(tempfile.mkdtemp(prefix="prism-uno-bootstrap-"))
    canary, profile = owned / "canary", owned / "한글 request $"
    canary.mkdir()
    profile.mkdir()
    (canary / "bad-services.rdb").write_bytes(b"Owned invalid registry fixture.")
    registry = (
        canary
        / "LibreOffice/4/user/uno_packages/cache/registry/com.sun.star.comp.deployment.component.PackageRegistryBackend"
    )
    registry.mkdir(parents=True)
    (registry / "unorc").write_text(
        "[Bootstrap]\nUNO_SERVICES=" + (canary / "bad-services.rdb").as_uri() + "\n",
        encoding="utf-8",
    )
    private_user = profile / "user/user"
    private_user.mkdir(parents=True)
    (private_user / "registrymodifications.xcu").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<oor:items xmlns:oor="http://openoffice.org/2001/registry">'
        '<item oor:path="/org.openoffice.Setup/L10N">'
        '<prop oor:name="ooLocale" oor:op="fuse"><value>ko-KR</value></prop>'
        "</item></oor:items>",
        encoding="utf-8",
    )
    before = {
        str(file.relative_to(canary)): digest(file)
        for file in canary.rglob("*")
        if file.is_file()
    }
    system = Path(os.environ["SystemRoot"])
    environment = {
        "SystemRoot": str(system),
        "WINDIR": str(system),
        "PATH": os.pathsep.join(
            map(
                str,
                [
                    runtime / "program",
                    (runtime / catalog["executable"]).parent,
                    system / "System32",
                ],
            )
        ),
        "TEMP": str(owned),
        "TMP": str(owned),
        # The control must NEVER resolve the actual user's normal profile.
        "SYSUSERCONFIG": canary.as_uri(),
    }
    results = {}
    for mode in ["control", "private"]:
        child = subprocess.run(
            [
                str(runtime / catalog["executable"]),
                "-I",
                "-S",
                "-B",
                str(Path(__file__).resolve()),
                "--probe",
                str(runtime),
                str(profile),
                mode,
            ],
            env=environment,
            cwd=owned,
            capture_output=True,
            check=False,
            timeout=30,
            creationflags=0x08000000,
        )
        (owned / f"{mode}-stderr.bin").write_bytes(child.stderr)
        assert child.returncode == 0 and not child.stderr, (mode, child.returncode)
        results[mode] = json.loads(child.stdout)
    assert results["control"] == {"bootstrapOK": False, "canaryError": True}, results
    assert results["private"] == {
        "bootstrapOK": True,
        "privateUser": True,
        "privatePackages": True,
        "hostRegistryLayer": False,
        "canaryConfiguration": False,
        "privateLocale": True,
    }, results
    after = {
        str(file.relative_to(canary)): digest(file)
        for file in canary.rglob("*")
        if file.is_file()
    }
    assert before == after, "The outside canary must remain unchanged"
    record = {
        "passed": True,
        "directory": str(owned),
        "verifiedEngineFiles": len(inventory),
        "workerSha256": digest(ROOT / "worker.py"),
        "canaryUnchanged": True,
        "results": results,
    }
    (owned / "result.json").write_text(json.dumps(record, indent=2), encoding="utf-8")
    print(json.dumps(record))


if __name__ == "__main__":
    main()
