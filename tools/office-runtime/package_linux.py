"""Maintainer-only Linux runtime archive from trusted official extractions.

Upstream hashes authenticate the source archives, not arbitrary prepared trees.
No package install/scripts run. All links are materialized within the supplied
trusted roots; the application accepts only the resulting compiled catalog.
"""

import argparse
import hashlib
import json
import shutil
import stat
import struct
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

from package_windows import digest, relative, verify_archive

ROOT = Path(__file__).resolve().parent


def elf_machine(path):
    with path.open("rb") as source:
        header = source.read(20)
    if len(header) != 20 or header[:6] != b"\x7fELF\x02\x01":
        raise ValueError("Expected little-endian ELF64")
    return struct.unpack_from("<H", header, 18)[0]


def mapping(runtime, dependencies, fonts):
    files, seen = {}, set()

    def add(path, name):
        relative(name)
        if name.casefold() in seen or name == "inventory.json":
            raise ValueError("Duplicate or reserved member")
        seen.add(name.casefold())
        files[name] = path

    def walk(directory, prefix, root, ancestors=frozenset()):
        real = directory.resolve(strict=True)
        if not real.is_relative_to(root) or real in ancestors:
            raise ValueError("Link escapes or cycles")
        for path in sorted(directory.iterdir()):
            target = path.resolve(strict=True)
            if not target.is_relative_to(root):
                raise ValueError("Link escapes source")
            name = prefix + path.name
            if path.is_dir():
                walk(path, name + "/", root, ancestors | {real})
            elif path.is_file():
                add(target, name)
            else:
                raise ValueError("Non-regular source member")

    walk(runtime, "", runtime.resolve(strict=True))
    libraries = dependencies / "usr/lib/x86_64-linux-gnu"
    walk(libraries, "program/prism-libs/", libraries.resolve(strict=True))
    documents = dependencies / "usr/share/doc"
    for name in ["libnss3", "libnspr4", "libsqlite3-0"]:
        directory = documents / name
        walk(
            directory,
            f"share/prism-dependency-notices/{name}/",
            documents.resolve(strict=True),
        )
    for source in sorted(fonts.iterdir()):
        name = source.name
        if source.suffix.lower() in {".ttf", ".otf"}:
            add(source, "share/fonts/truetype/Prism-" + name)
        else:
            add(source, "share/prism-font-notices/" + name)
    for name in [
        "program/soffice.bin",
        "program/python.bin",
        "program/pyuno.so",
        "program/libpython3.13.so.1.0",
    ]:
        if elf_machine(files[name]) != 62:
            raise ValueError("Expected x86-64 runtime")
    for name in ["LICENSE", "LICENSE.html", "NOTICE", "CREDITS.fodt"]:
        if name not in files:
            raise ValueError("Missing source notice")
    return dict(sorted(files.items()))


def inventory(files):
    entries = [
        {
            "path": name,
            "bytes": path.stat().st_size,
            "sha256": digest(path),
            "unixMode": 0o555 if path.stat().st_mode & stat.S_IXUSR else 0o444,
        }
        for name, path in files.items()
    ]
    if len(entries) > 60_000 or sum(p["bytes"] for p in entries) > 4 << 30:
        raise ValueError("Runtime exceeds catalog budgets")
    return entries, json.dumps(
        entries, ensure_ascii=False, separators=(",", ":")
    ).encode()


def archive(path, files, entries, raw):
    modes = {entry["path"]: entry["unixMode"] for entry in entries}
    with ZipFile(
        path, "x", compression=ZIP_DEFLATED, compresslevel=6, allowZip64=True
    ) as output:
        for name, source in [*files.items(), ("inventory.json", None)]:
            info = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | modes.get(name, 0o444)) << 16
            info.compress_type = ZIP_DEFLATED
            if source is None:
                output.writestr(info, raw)
            else:
                with (
                    source.open("rb") as reader,
                    output.open(info, "w", force_zip64=True) as writer,
                ):
                    shutil.copyfileobj(reader, writer, 1 << 20)
    verify_archive(path, entries, raw)
    with ZipFile(path) as output:
        for entry in entries:
            if (
                output.getinfo(entry["path"]).external_attr >> 16
            ) != stat.S_IFREG | entry["unixMode"]:
                raise ValueError("Executable mode mismatch")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--dependencies", required=True, type=Path)
    parser.add_argument("--dependency-archives", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--lock", required=True, type=Path)
    args = parser.parse_args()
    source = json.loads((ROOT / "linux-source.lock.json").read_text(encoding="utf-8"))
    if (
        args.upstream.stat().st_size != source["source"]["bytes"]
        or digest(args.upstream) != source["source"]["sha256"]
    ):
        raise ValueError("Upstream archive authentication failed")
    for dependency in source["dependencies"]:
        path = args.dependency_archives / dependency["url"].rsplit("/", 1)[1]
        if (
            path.stat().st_size != dependency["bytes"]
            or digest(path) != dependency["sha256"]
        ):
            raise ValueError("Dependency archive authentication failed")
    if args.output.exists() or args.lock.exists():
        raise ValueError("Outputs must be fresh paths")
    fonts = ROOT.parent.parent / "src-tauri/assets/document-fonts"
    for entry in json.loads((fonts / "sources.json").read_text(encoding="utf-8")):
        path = fonts / entry["file"]
        if path.stat().st_size != entry["bytes"] or digest(path) != entry["sha256"]:
            raise ValueError("Font authentication failed")
    files = mapping(
        args.runtime.resolve(strict=True), args.dependencies.resolve(strict=True), fonts
    )
    entries, raw = inventory(files)
    archive(args.output, files, entries, raw)
    catalog = {
        "schema": 2,
        "target": "linux-x64",
        "engine": "LibreOffice",
        "version": source["version"],
        "executable": source["interpreter"],
        "enginePath": source["engine"],
        "pythonHome": source["pythonHome"],
        "inventorySha256": hashlib.sha256(raw).hexdigest(),
        "fileCount": len(entries),
        "expandedBytes": sum(p["bytes"] for p in entries),
        "archive": {
            "bytes": args.output.stat().st_size,
            "sha256": digest(args.output),
            "url": None,
        },
        "upstreamSha256": source["source"]["sha256"],
    }
    args.lock.write_text(json.dumps(catalog, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(catalog))


if __name__ == "__main__":
    main()
