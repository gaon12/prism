"""Maintainer-only deterministic archive from an independently verified MSI extraction.

No Windows Installer execution. The extracted tree is a trusted maintainer input,
not an arbitrary user folder authenticated merely by possessing the source MSI.
"""

import argparse
import hashlib
import json
import shutil
import stat
import struct
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

ROOT = Path(__file__).resolve().parent
INVENTORY = "inventory.json"
CRT_NAMES = {
    "concrt140.dll",
    "msvcp140.dll",
    "msvcp140_1.dll",
    "msvcp140_2.dll",
    "msvcp140_atomic_wait.dll",
    "msvcp140_codecvt_ids.dll",
    "vccorlib140.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "vcruntime140_threads.dll",
}


def machine(path):
    with path.open("rb") as source:
        header = source.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            raise ValueError("Expected a PE component")
        offset = struct.unpack_from("<I", header, 60)[0]
        if offset > 1 << 20:
            raise ValueError("Invalid PE header offset")
        source.seek(offset)
        header = source.read(6)
        if len(header) != 6 or header[:4] != b"PE\x00\x00":
            raise ValueError("Invalid PE header")
        return struct.unpack_from("<H", header, 4)[0]


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            value.update(chunk)
    return value.hexdigest()


def relative(value):
    if (
        not value
        or len(value) > 2048
        or any(ord(c) < 32 or c in '\\:<>"|?*' for c in value)
    ):
        raise ValueError("Unsafe package member")
    for part in value.split("/"):
        stem = part.split(".", 1)[0].upper()
        if (
            not part
            or part in {".", ".."}
            or part.endswith((".", " "))
            or stem in {"CON", "PRN", "AUX", "NUL"}
            or stem
            in {f"{prefix}{i}" for prefix in ["COM", "LPT"] for i in range(1, 10)}
        ):
            raise ValueError("Unsafe package member")
    return value


def source_files(directory):
    for path in sorted(directory.rglob("*")):
        metadata = path.lstat()
        if stat.S_ISDIR(metadata.st_mode):
            continue
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("Only ordinary source files are supported")
        yield path, path.relative_to(directory).as_posix()


def mapping(application, crt, document_fonts):
    files = {}
    folded = set()

    def add(source, name, duplicate_same=False):
        name = relative(name)
        if name.casefold() in folded:
            if (
                duplicate_same
                and name in files
                and digest(files[name]) == digest(source)
            ):
                return
            raise ValueError("Duplicate package destination")
        folded.add(name.casefold())
        files[name] = source

    # msitools retains target:source in the MSI root's spelling. No colon from
    # that extraction-only directory is copied to the resulting Windows paths.
    for source, name in source_files(application):
        add(source, name)
    for source, name in source_files(application.parent / "Fonts"):
        add(source, "share/fonts/truetype/" + name, True)
    # Different architecture merge modules share the System64 target directory.
    # Flattening their long filenames overwrites amd64 files with ARM64 files.
    # Extract CAB members by their original *_amd64.<guid> MSI File identifiers.
    selected = {}
    for source, member in source_files(crt):
        if "/" in member or "_amd64." not in member:
            raise ValueError("Expected original amd64 MSI File identifier")
        name = member.split("_amd64.", 1)[0]
        if name not in CRT_NAMES or name in selected or machine(source) != 0x8664:
            raise ValueError("Invalid or non-x64 CRT component")
        selected[name] = source
    if set(selected) != CRT_NAMES:
        raise ValueError("Incomplete amd64 CRT")
    for name, source in selected.items():
        add(source, "program/" + name, True)
        # An executable's own directory takes precedence over system DLL paths.
        # Keep both the engine and direct Python interpreter app-local.
        add(source, "program/python-core-3.13.17/bin/" + name, True)
    # Keep python313.dll in its original program directory. Duplicating that
    # interpreter DLL next to the executable can load two interpreter instances
    # when pyuno resolves its own dependencies.
    for source, name in source_files(document_fonts):
        if name.endswith((".otf", ".ttf")):
            add(source, "share/fonts/truetype/Prism-" + name)
        else:
            add(source, "share/prism-font-notices/" + name)
    for required in [
        "program/soffice.com",
        "program/soffice.bin",
        "program/python-core-3.13.17/bin/python.exe",
        "license.txt",
        "LICENSE.html",
        "NOTICE",
        "CREDITS.fodt",
    ]:
        if required not in files:
            raise ValueError("Required engine or notice missing")
    for name in [
        "program/soffice.com",
        "program/soffice.bin",
        "program/pyuno.pyd",
        "program/python313.dll",
        "program/python-core-3.13.17/bin/python.exe",
    ]:
        if machine(files[name]) != 0x8664:
            raise ValueError("Unexpected engine/interpreter architecture")
    return dict(sorted(files.items()))


def inventory(files):
    entries = [
        {"path": name, "bytes": path.stat().st_size, "sha256": digest(path)}
        for name, path in files.items()
    ]
    return entries, json.dumps(
        entries, ensure_ascii=False, separators=(",", ":")
    ).encode("utf-8")


def archive(path, files, inventory_bytes):
    with ZipFile(
        path, "x", compression=ZIP_DEFLATED, compresslevel=6, allowZip64=True
    ) as output:
        for name, source in [*files.items(), (INVENTORY, None)]:
            metadata = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            metadata.create_system = 3
            metadata.external_attr = (stat.S_IFREG | 0o644) << 16
            metadata.compress_type = ZIP_DEFLATED
            if source is None:
                output.writestr(metadata, inventory_bytes)
            else:
                with (
                    source.open("rb") as reader,
                    output.open(metadata, "w", force_zip64=True) as writer,
                ):
                    shutil.copyfileobj(reader, writer, 1 << 20)


def verify_archive(path, entries, inventory_bytes):
    with ZipFile(path) as archive:
        if archive.namelist() != [entry["path"] for entry in entries] + [INVENTORY]:
            raise ValueError("Unexpected archive order or members")
        if archive.read(INVENTORY) != inventory_bytes:
            raise ValueError("Changed inventory")
        for entry in entries:
            info = archive.getinfo(entry["path"])
            if info.file_size != entry["bytes"]:
                raise ValueError("Source changed while packaging")
            value = hashlib.sha256()
            with archive.open(info) as source:
                for chunk in iter(lambda: source.read(1 << 20), b""):
                    value.update(chunk)
            if value.hexdigest() != entry["sha256"]:
                raise ValueError("Source changed while packaging")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--msi", type=Path, required=True)
    parser.add_argument("--extracted", type=Path, required=True)
    parser.add_argument("--crt", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--lock", type=Path, required=True)
    args = parser.parse_args()
    source = json.loads((ROOT / "windows-source.lock.json").read_text(encoding="utf-8"))
    if (
        args.msi.stat().st_size != source["source"]["bytes"]
        or digest(args.msi) != source["source"]["sha256"]
    ):
        raise ValueError("Upstream MSI authentication failed")
    if args.output.exists() or args.lock.exists():
        raise ValueError("Outputs must be fresh paths")
    fonts = ROOT.parent.parent / "src-tauri/assets/document-fonts"
    for entry in json.loads((fonts / "sources.json").read_text(encoding="utf-8")):
        path = fonts / entry["file"]
        if path.stat().st_size != entry["bytes"] or digest(path) != entry["sha256"]:
            raise ValueError("Bundled font source authentication failed")
    files = mapping(
        args.extracted.resolve(strict=True) / "LibreOffice:.",
        args.crt.resolve(strict=True),
        fonts,
    )
    entries, raw = inventory(files)
    archive(args.output, files, raw)
    verify_archive(args.output, entries, raw)
    value = {
        "schema": 1,
        "target": "windows-x64",
        "engine": "LibreOffice",
        "version": source["version"],
        "executable": source["interpreter"],
        "enginePath": source["engine"],
        "inventorySha256": hashlib.sha256(raw).hexdigest(),
        "fileCount": len(entries),
        "expandedBytes": sum(entry["bytes"] for entry in entries),
        "archive": {
            "bytes": args.output.stat().st_size,
            "sha256": digest(args.output),
            "url": None,
        },
        "upstreamMsiSha256": source["source"]["sha256"],
    }
    args.lock.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(value))


if __name__ == "__main__":
    main()
