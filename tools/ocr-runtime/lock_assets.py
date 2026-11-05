"""Maintainer tool: pin official artifacts and the exact installed inventory.

Users never run pip or this tool. The app downloads only these pinned artifacts,
extracts wheels without executing installers, and verifies this inventory digest
before launching its private interpreter. --materialize creates a NEW QA folder.
"""
from __future__ import annotations
import argparse
import concurrent.futures
import email.parser
import hashlib
import json
import shutil
import tarfile
import urllib.parse
import urllib.request
import zipfile
from html.parser import HTMLParser
from pathlib import Path

PYTHON_VERSION = "3.13.17"
PYTHON_SHA256 = "9e35319bffedab8b36ad44d8f377cb4f1733bcec257a0b6d04d94f01486ddb38"
PYTHON_BYTES = 11_441_267
BOOTSTRAP = b"python313.zip\n.\nLib/site-packages\n"
TARGET = "windows-x64"
LINUX_PYTHON_VERSION = "3.13.18"
LINUX_PYTHON_NAME = "cpython-3.13.18+20261026-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz"
LINUX_PYTHON_BYTES = 35_395_906
LINUX_PYTHON_SHA256 = "df15fe60d3ff55bf9dcf25824b5572756bb8816ff93865a273d3301d7066a8b7"
# -I -S excludes user modules and .pth execution. Add only the private wheel tree
# before running the same worker as Windows; stdlib stays interpreter-relative.
LINUX_BOOTSTRAP = b'''import os, runpy, sys
root = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(root, "python", "lib", "python3.13", "site-packages"))
sys.argv[0] = os.path.join(root, "worker.py")
runpy.run_path(sys.argv[0], run_name="__main__")
'''


def safe_path(value: str) -> str:
    if (not value or len(value) > 2048 or "\\" in value or ":" in value
            or any(ord(c) < 32 or ord(c) == 127 for c in value)
            or value.startswith("/") or value.endswith("/")
            or any(p in ("", ".", "..") or p.endswith((".", " ")) for p in value.split("/"))):
        raise ValueError("unsafe artifact member")
    return value


def destination(kind: str, member: str, target: str = TARGET) -> str:
    member = safe_path(member)
    if kind == "python":
        if target == "linux-x64":
            if not member.startswith("python/"):
                raise ValueError("interpreter member outside python prefix")
            return member
        return "python/" + member
    site = "python/lib/python3.13/site-packages" if target == "linux-x64" else "python/Lib/site-packages"
    parts = member.split("/")
    if parts[0].endswith(".data"):
        if len(parts) < 3:
            raise ValueError("invalid wheel data scheme")
        base = {"purelib": site, "platlib": site, "data": "python",
                "headers": "python/include", "scripts": "python/bin" if target == "linux-x64" else "python/Scripts"}[parts[1]]
        return base + "/" + "/".join(parts[2:])
    return site + "/" + member


def alias_target(name: str, link: str, hard: bool) -> str:
    """Resolve archive links lexically; never create or follow filesystem links."""
    safe_path(name)
    if (not link or link.startswith("/") or "\\" in link or ":" in link
            or any(ord(c) < 32 or ord(c) == 127 for c in link)):
        raise ValueError("unsafe interpreter alias")
    parts = [] if hard else name.split("/")[:-1]
    for part in link.split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            if len(parts) <= 1:
                raise ValueError("alias escapes interpreter")
            parts.pop()
        else:
            parts.append(part)
    result = safe_path("/".join(parts))
    if not result.startswith("python/"):
        raise ValueError("alias escapes interpreter")
    return result


def unpack_python_tar(path: Path, register, root: Path | None):
    files, aliases = {}, {}
    with tarfile.open(path, "r:gz") as archive:
        for member in archive:
            if member.isdir():
                safe_path(member.name.rstrip("/"))
                continue
            name = destination("python", member.name, "linux-x64")
            if name in files or name in aliases:
                raise ValueError("duplicate interpreter member")
            if member.issym() or member.islnk():
                aliases[name] = alias_target(name, member.linkname, member.islnk())
            elif member.isfile():
                sha = hashlib.sha256()
                output = None
                if root:
                    dest = root / name
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    output = dest.open("xb")
                try:
                    with archive.extractfile(member) as source:
                        while chunk := source.read(1 << 20):
                            sha.update(chunk)
                            if output:
                                output.write(chunk)
                finally:
                    if output:
                        output.close()
                files[name] = dict(path=name, bytes=member.size, sha256=sha.hexdigest())
                register(name, member.size, sha.hexdigest())
            else:
                raise ValueError("unsupported interpreter member type")
    resolved = []
    for name, target in aliases.items():
        visited = {name}
        while target in aliases:
            if target in visited:
                raise ValueError("cyclic interpreter alias")
            visited.add(target)
            target = aliases[target]
        item = files[target]
        resolved.append((name, target, item))
    for name, target, item in resolved:
        register(name, item["bytes"], item["sha256"])
        if root:
            dest = root / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            with (root / target).open("rb") as source, dest.open("xb") as output:
                shutil.copyfileobj(source, output)
    if root:
        (root / "python/bin/python3.13").chmod(0o755)


def file_hash(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.links.extend(value for key, value in attrs if key == "href")


def official_wheel(path: Path) -> dict:
    with zipfile.ZipFile(path) as archive:
        metadata = next(f for f in archive.namelist() if f.endswith(".dist-info/METADATA"))
        info = email.parser.BytesParser().parsebytes(archive.read(metadata))
    name, version = info["Name"], info["Version"]
    digest = file_hash(path)
    if name.lower() in ("torch", "torchvision"):
        base = "https://download.pytorch.org/whl/cpu/" + name.lower() + "/"
        parser = Links()
        parser.feed(urllib.request.urlopen(base, timeout=60).read().decode("utf-8"))
        selected = next(url for url in parser.links if urllib.parse.unquote(urllib.parse.urlsplit(url).path.rsplit("/", 1)[-1]) == path.name)
        url, fragment = urllib.parse.urldefrag(urllib.parse.urljoin(base, selected))
        if fragment != "sha256=" + digest:
            raise ValueError("PyTorch index hash mismatch")
    else:
        api = "https://pypi.org/pypi/" + urllib.parse.quote(name) + "/" + urllib.parse.quote(version) + "/json"
        data = json.load(urllib.request.urlopen(api, timeout=60))
        release = next(f for f in data["urls"] if f["filename"] == path.name)
        if release["digests"]["sha256"] != digest or release["size"] != path.stat().st_size:
            raise ValueError("PyPI hash/length mismatch")
        url = release["url"]
    if urllib.parse.urlsplit(url).scheme != "https":
        raise ValueError("non-HTTPS artifact")
    return dict(kind="wheel", path="wheels/" + path.name, url=url,
                bytes=path.stat().st_size, sha256=digest, package=name, version=version)


def digest_inventory(files: list[dict]) -> str:
    digest = hashlib.sha256()
    for item in sorted(files, key=lambda item: item["path"]):
        digest.update((item["path"] + "\0" + str(item["bytes"]) + "\0" + item["sha256"] + "\n").encode("utf-8"))
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--wheelhouse", type=Path, required=True)
    parser.add_argument("--python-archive", type=Path, required=True)
    parser.add_argument("--model-manifest", type=Path, required=True)
    parser.add_argument("--model-directory", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--materialize", type=Path)
    parser.add_argument("--target", choices=(TARGET, "linux-x64"), default=TARGET)
    args = parser.parse_args()
    linux = args.target == "linux-x64"
    python_bytes = LINUX_PYTHON_BYTES if linux else PYTHON_BYTES
    python_sha = LINUX_PYTHON_SHA256 if linux else PYTHON_SHA256
    python_version = LINUX_PYTHON_VERSION if linux else PYTHON_VERSION
    bootstrap = LINUX_BOOTSTRAP if linux else BOOTSTRAP
    bootstrap_path = "bootstrap.py" if linux else "python/python313._pth"
    if (args.python_archive.stat().st_size != python_bytes or file_hash(args.python_archive) != python_sha
            or (linux and args.python_archive.name != LINUX_PYTHON_NAME)):
        raise ValueError("unexpected interpreter")
    root = args.materialize.resolve() if args.materialize else None
    if root:
        root.mkdir(parents=True, exist_ok=False)
    wheels = sorted(args.wheelhouse.glob("*.whl"))
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        wheel_assets = list(pool.map(official_wheel, wheels))
    assets = [dict(kind="pythonTarGz" if linux else "python", path=args.python_archive.name,
                   url=("https://github.com/astral-sh/python-build-standalone/releases/download/20261026/"
                        + urllib.parse.quote(LINUX_PYTHON_NAME) if linux else
                        f"https://www.python.org/ftp/python/{PYTHON_VERSION}/{args.python_archive.name}"),
                   bytes=python_bytes, sha256=python_sha)] + wheel_assets
    inventory = []
    seen = set()

    def register(name: str, size: int, sha: str):
        name = safe_path(name)
        key = name if linux else name.lower()
        if key in seen:
            raise ValueError("installed artifact collision: " + name)
        seen.add(key)
        inventory.append(dict(path=name, bytes=size, sha256=sha))

    def write_bytes(name: str, data: bytes):
        register(name, len(data), hashlib.sha256(data).hexdigest())
        if root:
            dest = root / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            with dest.open("xb") as output:
                output.write(data)

    for kind, path in [("python", args.python_archive)] + [("wheel", p) for p in wheels]:
        if linux and kind == "python":
            unpack_python_tar(path, register, root)
            continue
        with zipfile.ZipFile(path) as archive:
            for member in archive.infolist():
                if member.is_dir():
                    continue
                if kind == "python" and member.filename.endswith("._pth"):
                    continue
                if (member.external_attr >> 16) & 0o170000 == 0o120000:
                    raise ValueError("artifact links are not supported")
                name = destination(kind, member.filename, args.target)
                sha = hashlib.sha256()
                output = None
                if root:
                    dest = root / name
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    output = dest.open("xb")
                try:
                    with archive.open(member) as source:
                        while chunk := source.read(1 << 20):
                            sha.update(chunk)
                            if output:
                                output.write(chunk)
                finally:
                    if output:
                        output.close()
                register(name, member.file_size, sha.hexdigest())
    write_bytes(bootstrap_path, bootstrap)
    worker = Path(__file__).with_name("worker.py").read_bytes()
    write_bytes("worker.py", worker)
    models = json.loads(args.model_manifest.read_text(encoding="utf-8"))
    for item in models["files"]:
        name = "models/" + safe_path(item["path"])
        source = args.model_directory / item["path"]
        if source.stat().st_size != item["size"] or file_hash(source) != item["sha256"]:
            raise ValueError("model hash/length mismatch")
        register(name, item["size"], item["sha256"])
        assets.append(dict(kind="model", path=name, bytes=item["size"], sha256=item["sha256"], url=item["url"]))
        if root:
            dest = root / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, dest)
    manifest = dict(schema=1, target=args.target, pipeline="PaddleOCR-VL-1.6", pythonVersion=python_version,
                    executable="python/bin/python3.13" if linux else "python/python.exe", bootstrapPath=bootstrap_path,
                    bootstrap=bootstrap.decode(), workerSha256=hashlib.sha256(worker).hexdigest(),
                    inventorySha256=digest_inventory(inventory), fileCount=len(inventory),
                    expandedBytes=sum(f["bytes"] for f in inventory), assets=assets)
    if linux:
        manifest["bootstrapEntrypoint"] = True
    args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    if root:
        (root / "inventory.json").write_text(json.dumps(inventory, separators=(",", ":")), encoding="utf-8")
    print(json.dumps(dict(target=args.target, assets=len(assets), files=len(inventory),
                          expandedBytes=manifest["expandedBytes"], materialized=str(root))))


if __name__ == "__main__":
    main()
