"""Real Linux Tauri/WebKitGTK smoke check; no page, media or IPC responses replaced."""

import argparse
import base64
import hashlib
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def program(value):
    located = shutil.which(value)
    if not located:
        raise argparse.ArgumentTypeError(f"Executable not found: {value}")
    return str(Path(located).resolve(strict=True))


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--executable",
    required=True,
    type=program,
    help="Bundled Prism executable or AppImage",
)
parser.add_argument(
    "--artifacts",
    required=True,
    type=Path,
    help="Existing directory for a new, retained run folder",
)
parser.add_argument(
    "--profile-parent",
    type=Path,
    help="Existing native Linux filesystem directory, when artifacts use a mounted filesystem",
)
parser.add_argument(
    "--expect-release",
    action="store_true",
    help="Require a genuine release build using a fresh installed XDG profile",
)
parser.add_argument(
    "--expected-sha256",
    help="Pinned executable/AppImage SHA256; required with --expect-release",
)
parser.add_argument("--driver", default="tauri-driver", type=program)
parser.add_argument("--native-driver", default="WebKitWebDriver", type=program)
parser.add_argument("--xvfb", default="Xvfb", type=program)
parser.add_argument("--window-manager", default="openbox", type=program)
parser.add_argument("--window-manager-config", type=Path)
parser.add_argument("--xprop", default="xprop", type=program)
parser.add_argument(
    "--runtime-temp-parent",
    type=Path,
    help="Optional existing filesystem for a fresh runtime temp directory (for example private tmpfs)",
)
parser.add_argument(
    "--font", type=Path, help="Optional CJK font copied only into this run’s XDG data"
)
args = parser.parse_args()
if sys.platform != "linux":
    parser.error("This check needs actual Linux WebKitGTK, Xvfb and Openbox.")
root = args.artifacts.resolve(strict=True)
if not root.is_dir():
    parser.error("--artifacts must be an existing directory")
repo = Path(__file__).resolve().parents[2]
if root.is_relative_to(repo) and not root.is_relative_to(repo / ".dev"):
    parser.error(
        "Keep run artifacts outside the checkout or inside its ignored .dev directory"
    )
profile_parent = (
    args.profile_parent.resolve(strict=True) if args.profile_parent else root
)
if not profile_parent.is_dir():
    parser.error("--profile-parent must be an existing directory")
candidate = Path(args.executable)
script_sha = file_hash(Path(__file__))
app_sha = file_hash(candidate)
if args.expect_release and not args.expected_sha256:
    parser.error("--expect-release requires --expected-sha256")
if args.expected_sha256 and not re.fullmatch(r"[a-fA-F0-9]{64}", args.expected_sha256):
    parser.error("--expected-sha256 must be a SHA256 hex digest")
if args.expected_sha256 and app_sha != args.expected_sha256.lower():
    parser.error("The selected executable does not match --expected-sha256")

fixture = Path(tempfile.mkdtemp(prefix="prism-linux-native-", dir=root))
(fixture / "qa-owner.txt").write_text(
    "Prism native Linux GUI QA v1\n", encoding="utf-8"
)
profile = Path(tempfile.mkdtemp(prefix="prism-linux-profile-", dir=profile_parent))
(profile / "qa-owner.txt").write_text(
    "Prism native Linux GUI profile v1\n", encoding="utf-8"
)
(fixture / "profile-location.txt").write_text(str(profile) + "\n", encoding="utf-8")
temporary = profile / "tmp"
runtime_temp_identity = None
if args.runtime_temp_parent:
    parent = args.runtime_temp_parent.resolve(strict=True)
    if not parent.is_dir():
        parser.error("--runtime-temp-parent must be an existing directory")
    temporary = Path(tempfile.mkdtemp(prefix="prism-linux-runtime-", dir=parent))
    (temporary / "qa-owner.txt").write_bytes(b"Prism native Linux runtime temp v1\n")
    (fixture / "runtime-temp-location.txt").write_text(
        str(temporary) + "\n", encoding="utf-8"
    )
    runtime_stat = temporary.stat()
    runtime_temp_identity = (runtime_stat.st_dev, runtime_stat.st_ino)
temporary.mkdir(exist_ok=True)
if args.font:
    font_source = args.font.resolve(strict=True)
    font_directory = profile / "data/fonts"
    font_directory.mkdir(parents=True)
    font_target = font_directory / font_source.name
    shutil.copyfile(font_source, font_target)
    assert file_hash(font_source) == file_hash(font_target)
files = fixture / "files"
files.mkdir()
source = files / "한국어 소설.txt"
original = (
    "제1장 한국어 日本語 中文 😀\n<script>window.BAD_TEXT_SCRIPT=true</script>\n"
    + "첫 페이지 원문을 실제 Linux WebKitGTK에서 읽습니다.\n" * 120
)
source.write_text(original)
(files / "예산 보고서.md").write_text("# 예산 보고서\n부산 출장 경비 180000원")
settings_dir = profile / "config/app.prism.desktop" if args.expect_release else profile
settings_dir.mkdir(parents=True, exist_ok=True)
(settings_dir / "settings.json").write_text(
    json.dumps(
        {
            "onboardingVersion": 1,
            "language": "ko",
            "theme": "light",
            "startOnLogin": False,
            "shortcut": "Ctrl+Alt+Shift+F9",
            "updates": {"autoCheck": False},
            "clipboard": {"enabled": False, "onboarded": True},
            "scopes": [
                {
                    "id": "fixture",
                    "name": "격리 자료",
                    "path": str(files),
                    "filenameEnabled": True,
                    "contentEnabled": False,
                    "externalAllowed": False,
                }
            ],
        }
    )
)
env = {
    **os.environ,
    "PRISM_DEV_DATA_DIR": str(profile),
    "PRISM_DEV_TEST_MODE": "1",
    "GDK_BACKEND": "x11",
    "TMPDIR": str(temporary),
    "TAURI_WEBVIEW_AUTOMATION": "true",
    "XDG_CONFIG_HOME": str(profile / "config"),
    "XDG_DATA_HOME": str(profile / "data"),
    "XDG_CACHE_HOME": str(profile / "cache"),
}

# Normal per-user font discovery, inside this run's XDG_DATA_HOME. No host font
# installation or special fontconfig override is needed by the real WebView.
# Both GTK and the real release clipboard adapter must stay on this owned X display.
env.pop("WAYLAND_DISPLAY", None)
env.pop("WAYLAND_SOCKET", None)
env["XDG_SESSION_TYPE"] = "x11"
if args.expect_release:
    env.pop("PRISM_DEV_DATA_DIR", None)
    env.pop("PRISM_DEV_TEST_MODE", None)
env.pop("FONTCONFIG_FILE", None)
env.pop("FONTCONFIG_PATH", None)
# Start a separate X server; never attach to the user's active desktop.
display = next(
    n
    for n in range(91, 110)
    if not Path(f"/tmp/.X{n}-lock").exists()
    and not Path(f"/tmp/.X11-unix/X{n}").exists()
)
env["DISPLAY"] = f":{display}"
xvfb = None
driver = None
wm = None
session = None
stage = "X display"
records = {}


def until(check, label, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if xvfb and xvfb.poll() is not None:
            raise RuntimeError("Owned Xvfb exited")
        if wm and wm.poll() is not None:
            raise RuntimeError("Owned window manager exited")
        value = check()
        if value:
            return value
        time.sleep(0.12)
    raise TimeoutError(label)


def free_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


port = free_port()
native_port = free_port()
while native_port == port:
    native_port = free_port()
endpoint = f"http://127.0.0.1:{port}"


def http(method, path, data=None):
    request = urllib.request.Request(
        endpoint + path,
        data=None if data is None else json.dumps(data).encode(),
        method=method,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=125) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(error.read().decode()) from error
    # W3C errors use an HTTP error status. A successful script may deliberately
    # return {error: ...} to preserve a real native IPC rejection for assertions.
    return result.get("value")


def command(method, path, data=None):
    return http(method, "/session/" + session + path, data)


def js(script, *args):
    return command("POST", "/execute/sync", {"script": script, "args": list(args)})


def async_js(script, *args):
    return command("POST", "/execute/async", {"script": script, "args": list(args)})


def invoke(name, args=None):
    result = async_js(
        """const [name,args,done]=arguments;
        window.__TAURI_INTERNALS__.invoke(name,args).then(value=>done({value})).catch(error=>done({error}));""",
        name,
        args or {},
    )
    if "error" in result:
        raise RuntimeError(json.dumps(result["error"]))
    return result.get("value")


element_key = "element-6066-11e4-a52e-4f735466cecf"


def element(selector):
    return command("POST", "/element", {"using": "css selector", "value": selector})[
        element_key
    ]


def wait_for_paint():
    # React can replace a gallery/detail view before WebKit has painted the
    # new hit-test geometry. Wait for real animation frames before native input.
    painted = async_js("""const done=arguments[arguments.length-1];let finished=false;
        const finish=value=>{if(!finished){finished=true;done(value)}};
        setTimeout(()=>finish(false),2000);
        requestAnimationFrame(()=>requestAnimationFrame(()=>finish(document.visibilityState==='visible')));""")
    assert painted is True, "Native input target was not painted"


def click(selector):
    wait_for_paint()
    command("POST", "/element/" + element(selector) + "/click", {})


def button(text):
    wait_for_paint()
    found = js(
        "return [...document.querySelectorAll('button')].filter(n=>n.getClientRects().length&&(n.getAttribute('aria-label')===arguments[0]||(!n.hasAttribute('aria-label')&&n.textContent.trim()===arguments[0]))).map(n=>n)",
        text,
    )
    assert len(found) == 1, (text, len(found))
    command("POST", "/element/" + found[0][element_key] + "/click", {})


def fill(selector, text):
    item = element(selector)
    command("POST", "/element/" + item + "/click", {})
    # WebKitGTK rejects empty send-keys strings, and its clear command does not
    # drive React's controlled input consistently. Use actual editing keys.
    command("POST", "/element/" + item + "/value", {"text": "\ue009a\ue000\ue003"})
    if text:
        command("POST", "/element/" + item + "/value", {"text": text})
    until(
        lambda: js(
            "return document.querySelector(arguments[0])?.value===arguments[1]",
            selector,
            text,
        ),
        "native input value",
    )


def screenshot(name):
    painted = async_js("""const done=arguments[arguments.length-1];let completed=false;
        const finish=value=>{if(!completed){completed=true;done(value)}};
        setTimeout(()=>finish({painted:false,visibility:document.visibilityState}),2000);
        document.fonts.ready.then(()=>requestAnimationFrame(()=>requestAnimationFrame(()=>setTimeout(()=>finish({painted:true}),150))));""")
    assert painted.get("painted"), painted
    (fixture / name).write_bytes(base64.b64decode(command("GET", "/screenshot")))


def install_observer():
    js("""window.__qaErrors=[];
        for(const level of ['error','warn']) {const original=console[level];console[level]=function(...args){window.__qaErrors.push({level,text:args.map(String).join(' ')});original.apply(this,args)}}
        addEventListener('error',e=>__qaErrors.push({level:'error',text:e.message}));
        addEventListener('unhandledrejection',e=>__qaErrors.push({level:'error',text:String(e.reason)}));""")


try:
    with (fixture / "xvfb.log").open("wb") as xlog:
        xvfb = subprocess.Popen(
            [
                args.xvfb,
                env["DISPLAY"],
                "-screen",
                "0",
                "1600x1000x24",
                "-nolisten",
                "tcp",
            ],
            env=env,
            stdout=xlog,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    until(lambda: Path(f"/tmp/.X11-unix/X{display}").exists(), "private X display")
    stage = "native window manager"
    wm_log = (fixture / "window-manager.log").open("wb")
    wm = subprocess.Popen(
        [
            args.window_manager,
            "--sm-disable",
            *(
                ["--config-file", str(args.window_manager_config.resolve(strict=True))]
                if args.window_manager_config
                else []
            ),
        ],
        env=env,
        stdout=wm_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    def wm_ready():
        result = subprocess.run(
            [args.xprop, "-root", "-notype", "_NET_SUPPORTING_WM_CHECK"],
            env=env,
            text=True,
            capture_output=True,
            timeout=3,
            check=False,
        )
        match = re.search(r"window id # (0x[0-9a-fA-F]+)", result.stdout)
        return result.returncode == 0 and match and int(match[1], 16) > 0

    until(wm_ready, "EWMH window manager")
    stage = "native WebDriver"
    log = (fixture / "driver.log").open("wb")
    driver = subprocess.Popen(
        [
            args.driver,
            "--port",
            str(port),
            "--native-port",
            str(native_port),
            "--native-driver",
            args.native_driver,
        ],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    def available():
        if driver.poll() is not None:
            raise RuntimeError("Owned driver exited")
        try:
            return http("GET", "/status")
        except (urllib.error.URLError, RuntimeError):
            return False

    until(available, "native WebDriver status")
    stage = "native session"
    created = http(
        "POST",
        "/session",
        {
            "capabilities": {
                "alwaysMatch": {"tauri:options": {"application": str(candidate)}}
            }
        },
    )
    session = created["sessionId"]
    records["capabilities"] = created.get("capabilities")
    command("POST", "/timeouts", {"script": 120000, "pageLoad": 120000, "implicit": 0})
    stage = "actual main and quick windows"

    def find_main():
        for handle in command("GET", "/window/handles"):
            command("POST", "/window", {"handle": handle})
            url = command("GET", "/url")
            if url.startswith("tauri://") and "#quick" not in url:
                return handle
        return None

    main = until(find_main, "bundled main native window")
    quick = None
    command("POST", "/window", {"handle": main})
    install_observer()
    assert command("GET", "/title") == "Prism"
    assert command("GET", "/url").startswith("tauri://")
    until(
        lambda: js(
            "return document.querySelector('.page-heading h1')?.textContent==='파일 검색'"
        ),
        "nonblank main heading",
    )
    runtime = invoke("runtime_info")
    records["runtime"] = runtime
    assert (
        runtime["platform"] == "linux"
        and runtime["native"]
        and runtime["development"] is (not args.expect_release)
    )
    assert (
        runtime["installKind"] == ("installed" if args.expect_release else "isolated")
        and runtime["startupError"] is None
    ), runtime
    actual_settings = invoke("get_settings")
    assert actual_settings["clipboard"]["enabled"] is False
    assert actual_settings["updates"]["autoCheck"] is False
    assert actual_settings["startOnLogin"] is False
    assert len(actual_settings["scopes"]) == 1 and actual_settings["scopes"][0][
        "path"
    ] == str(files)
    assert (
        actual_settings["scopes"][0]["contentEnabled"] is False
        and actual_settings["scopes"][0]["externalAllowed"] is False
    )
    stage = "real filename index and rendered Korean query"
    until(
        lambda: invoke("index_status")["initialScanComplete"], "initial fixture index"
    )
    button("파일명")
    fill('input[aria-label="검색어"]', "예산")
    until(
        lambda: js(
            "return [...document.querySelectorAll('.file-result-name')].some(n=>n.textContent.includes('예산 보고서.md'))"
        ),
        "Korean filename result",
    )
    records["filename"] = invoke(
        "search_files", {"query": "예산", "extension": "", "scopeId": "fixture"}
    )
    assert records["filename"]["total"] == 1
    screenshot("linux-filename.png")
    stage = "real Spotlight empty state and calculator"
    invoke("show_quick")
    if not quick:

        def find_quick():
            for handle in command("GET", "/window/handles"):
                command("POST", "/window", {"handle": handle})
                if "#quick" in command("GET", "/url"):
                    install_observer()
                    return handle
            return None

        quick = until(find_quick, "Spotlight native window")
    command("POST", "/window", {"handle": quick})
    fill('input[aria-label="검색어"]', "")
    until(
        lambda: js(
            "return !document.querySelector('.quick-panel.expanded,.quick-results,.quick-meta,.quick-empty') && window.innerHeight<=140"
        ),
        "collapsed empty Spotlight",
    )
    assert not js("return document.body.textContent.includes('검색 결과가 없어요')")
    screenshot("linux-spotlight-empty.png")
    fill('input[aria-label="검색어"]', "=48/2(9+3)")
    until(
        lambda: js(
            "return document.querySelector('.utility-value')?.textContent==='2'"
        ),
        "implicit multiplication result",
    )
    screenshot("linux-spotlight.png")
    fill('input[aria-label="검색어"]', "=1/3")
    until(
        lambda: js(
            "return document.querySelector('.accuracy-note')?.textContent.includes('반올림')"
        ),
        "rounding notice",
    )
    records["rounded"] = js(
        "return document.querySelector('.accuracy-note').textContent"
    )
    screenshot("linux-spotlight-rounded.png")
    invoke("hide_quick")
    command("POST", "/window", {"handle": main})
    stage = "real collection and source text"
    collection = invoke("collection_create", {"title": "Linux 책장", "description": ""})
    work = invoke(
        "collection_work_create",
        {
            "collectionId": collection["id"],
            "title": "한글 첫 페이지",
            "metadata": {
                "kind": "novel",
                "direction": "leftToRight",
                "authors": [],
                "tags": [],
                "description": "",
                "externalId": None,
            },
        },
    )
    unit = invoke(
        "collection_unit_create",
        {"workId": work["id"], "title": "제1권", "kind": "volume"},
    )
    unit = invoke(
        "collection_import",
        {"unitId": unit["id"], "expected": unit["revision"], "paths": [str(source)]},
    )
    pages = invoke(
        "collection_pages",
        {"unitId": unit["id"], "expected": unit["revision"], "offset": 0, "limit": 48},
    )["items"]
    assert len(pages) > 1
    epub_cases = []
    for filename, title, width, height in [
        ("spine.epub", "EPUB 원문 첫 장", 360, 480),
        ("svg.epub", "SVG 원문 첫 장", 200, 300),
    ]:
        epub_source = files / filename
        epub_bytes = ((repo / "tests/fixtures/epub") / filename).read_bytes()
        epub_source.write_bytes(epub_bytes)
        epub_work = invoke(
            "collection_work_create",
            {
                "collectionId": collection["id"],
                "title": title,
                "metadata": {
                    "kind": "novel",
                    "direction": "leftToRight",
                    "authors": [],
                    "tags": [],
                    "description": "",
                    "externalId": None,
                },
            },
        )
        epub_unit = invoke(
            "collection_unit_create",
            {"workId": epub_work["id"], "title": "첫 권", "kind": "volume"},
        )
        epub_unit = invoke(
            "collection_import",
            {
                "unitId": epub_unit["id"],
                "expected": epub_unit["revision"],
                "paths": [str(epub_source)],
            },
        )
        epub_pages = invoke(
            "collection_pages",
            {
                "unitId": epub_unit["id"],
                "expected": epub_unit["revision"],
                "offset": 0,
                "limit": 48,
            },
        )["items"]
        before_cover = invoke("collection_cover", {"workId": epub_work["id"]})
        assert before_cover.get("document") and not before_cover.get("dataUrl")
        assert before_cover["pageId"] == epub_pages[0]["id"]
        assert before_cover["sourceHash"] == hashlib.sha256(epub_bytes).hexdigest()
        if filename == "spine.epub":
            assert (
                len(epub_pages) == 2
                and epub_pages[0]["location"]["entry"] == "OPS/chapters/2-first.xhtml"
            )
            assert (
                "<script" not in before_cover["document"]["html"]
                and "127.0.0.1" not in before_cover["document"]["html"]
            )
        epub_cases.append(
            (epub_work, epub_pages, epub_source, epub_bytes, width, height)
        )
    stage = "real PDF pages through the packaged helper, without OCR"
    pdf_cases = []
    records["pdf"] = []

    def pdf_pixels(url):
        return async_js(
            """const [url,done]=arguments;const image=new Image();
            image.onload=()=>{const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
            const context=canvas.getContext('2d');context.drawImage(image,0,0);
            const rgba=context.getImageData(0,0,image.width,image.height).data;let ink=0,green=0;
            for(let n=0;n<rgba.length;n+=4){if(Math.min(rgba[n],rgba[n+1],rgba[n+2])<180)ink++;
                if(rgba[n]<30&&rgba[n+1]>200&&rgba[n+2]<30)green++}
            done({width:image.width,height:image.height,ink,green,
                top:[...context.getImageData(Math.floor(image.width/2),Math.floor(image.height/8),1,1).data],
                bottom:[...context.getImageData(Math.floor(image.width/2),Math.floor(7*image.height/8),1,1).data]})};
            image.onerror=()=>done({error:'decode'});image.src=url;""",
            url,
        )

    for filename, title, count in [
        ("crop-clockwise.pdf", "PDF 잘림과 회전", 1),
        ("mixed.pdf", "PDF 원본 네 페이지", 4),
    ]:
        pdf_source = files / filename
        pdf_original = (repo / "tests/fixtures/ocr" / filename).read_bytes()
        pdf_source.write_bytes(pdf_original)
        pdf_work = invoke(
            "collection_work_create",
            {
                "collectionId": collection["id"],
                "title": title,
                "metadata": {
                    "kind": "document",
                    "direction": "leftToRight",
                    "authors": [],
                    "tags": [],
                    "description": "",
                    "externalId": None,
                },
            },
        )
        pdf_unit = invoke(
            "collection_unit_create",
            {"workId": pdf_work["id"], "title": "PDF 원문", "kind": "document"},
        )
        pdf_unit = invoke(
            "collection_import",
            {
                "unitId": pdf_unit["id"],
                "expected": pdf_unit["revision"],
                "paths": [str(pdf_source)],
            },
        )
        pdf_pages = invoke(
            "collection_pages",
            {
                "unitId": pdf_unit["id"],
                "expected": pdf_unit["revision"],
                "offset": 0,
                "limit": 10,
            },
        )["items"]
        assert len(pdf_pages) == count and [
            page["location"]["number"] for page in pdf_pages
        ] == list(range(1, count + 1))
        assert all(page["location"]["kind"] == "pdf" for page in pdf_pages)
        stats = []
        media_hashes = []
        for page in pdf_pages:
            media = invoke(
                "collection_media", {"pageId": page["id"], "thumbnail": True}
            )
            assert (
                media["pageId"] == page["id"]
                and media["sourceHash"] == hashlib.sha256(pdf_original).hexdigest()
            )
            assert (
                media["dataUrl"].startswith("data:image/webp;base64,")
                and media["text"] is None
                and not media.get("document")
            )
            actual = base64.b64decode(media["dataUrl"].split(",", 1)[1])
            assert actual[:4] == b"RIFF" and actual[8:12] == b"WEBP"
            stats.append(pdf_pixels(media["dataUrl"]))
            media_hashes.append(hashlib.sha256(actual).hexdigest())
        if filename == "crop-clockwise.pdf":
            assert (stats[0]["width"], stats[0]["height"]) == (100, 200), stats
            assert (
                stats[0]["top"] == [255, 0, 0, 255]
                and stats[0]["bottom"] == [0, 0, 255, 255]
                and stats[0]["green"] == 0
            ), stats
        else:
            assert (
                stats[0]["ink"] > 10
                and stats[1]["ink"] > 1000
                and stats[2]["ink"] > 10
                and stats[3]["ink"] == 0
            ), stats
            assert media_hashes[0] != media_hashes[2]
        pdf_cover = invoke("collection_cover", {"workId": pdf_work["id"]})
        assert (
            pdf_cover["pageId"] == pdf_pages[0]["id"]
            and pdf_cover["sourceHash"] == hashlib.sha256(pdf_original).hexdigest()
        )
        assert pdf_source.read_bytes() == pdf_original
        pdf_cases.append(
            {
                "work": pdf_work,
                "pages": pdf_pages,
                "source": pdf_source,
                "original": pdf_original,
            }
        )
        records["pdf"].append(
            {
                "fixture": filename,
                "sourceSha256": hashlib.sha256(pdf_original).hexdigest(),
                "pageNumbers": [page["location"]["number"] for page in pdf_pages],
                "pixels": stats,
                "webpSha256": media_hashes,
                "originalUnchanged": True,
            }
        )
    assert invoke("content_status")["indexed"] == 0
    button("컬렉션")
    until(
        lambda: js(
            "return !!document.querySelector('.collection-work-card[aria-label=\"한글 첫 페이지\"]')"
        ),
        "collection work card",
    )
    until(
        lambda: invoke("collection_cover", {"workId": work["id"]}).get("dataUrl"),
        "native text cover admission",
    )
    cover = invoke("collection_cover", {"workId": work["id"]})
    assert cover["dataUrl"].startswith("data:image/webp;base64,")
    encoded = base64.b64decode(cover["dataUrl"].split(",", 1)[1])
    assert encoded[:4] == b"RIFF" and encoded[8:12] == b"WEBP"
    assert cover["pageId"] == pages[0]["id"]
    assert cover["sourceHash"] == hashlib.sha256(original.encode()).hexdigest()
    pixels = async_js(
        """const [url,done]=arguments;const image=new Image();
        image.onload=()=>{const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);const rgba=context.getImageData(0,0,image.width,image.height).data;
        let ink=0;for(let n=0;n<rgba.length;n+=4)if(Math.min(rgba[n],rgba[n+1],rgba[n+2])<180)ink++;
        done({width:image.width,height:image.height,ink})};image.onerror=()=>done({error:'decode'});image.src=url;""",
        cover["dataUrl"],
    )
    assert pixels.get("height") == 480 and pixels["ink"] > 1000, pixels
    stage = "actual EPUB and SVG first-page cover pixels"
    records["epub"] = []
    for epub_work, epub_pages, epub_source, epub_bytes, width, height in epub_cases:
        until(
            lambda work_id=epub_work["id"]: invoke(
                "collection_cover", {"workId": work_id}
            ).get("dataUrl"),
            "actual EPUB cover admission",
        )
        epub_cover = invoke("collection_cover", {"workId": epub_work["id"]})
        assert epub_cover["dataUrl"].startswith(
            "data:image/webp;base64,"
        ) and not epub_cover.get("document")
        actual = base64.b64decode(epub_cover["dataUrl"].split(",", 1)[1])
        assert actual[:4] == b"RIFF" and actual[8:12] == b"WEBP"
        assert (
            epub_cover["pageId"] == epub_pages[0]["id"]
            and epub_cover["sourceHash"] == hashlib.sha256(epub_bytes).hexdigest()
        )
        epub_pixel = async_js(
            """const [url,done]=arguments;const image=new Image();
            image.onload=()=>{const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
                const context=canvas.getContext('2d');context.drawImage(image,0,0);
                done({width:image.width,height:image.height,rgba:[...context.getImageData(Math.floor(image.width/2),Math.floor(image.height/2),1,1).data]})};
            image.onerror=()=>done({error:'decode'});image.src=url;""",
            epub_cover["dataUrl"],
        )
        assert (
            epub_pixel.get("width") == width and epub_pixel.get("height") == height
        ), epub_pixel
        assert all(
            abs(epub_pixel["rgba"][i] - color) <= 8
            for i, color in enumerate([210, 40, 50])
        ), epub_pixel
        assert epub_source.read_bytes() == epub_bytes
        records["epub"].append(
            {
                "title": epub_work["title"],
                "pageId": epub_cover["pageId"],
                "pixels": epub_pixel,
                "sourceUnchanged": True,
            }
        )
    screenshot("linux-collection.png")
    stage = "actual PDF reader and physical page bookmark"
    button("PDF 잘림과 회전")
    until(
        lambda: js(
            "return document.querySelectorAll('.collection-page-card').length===1"
        ),
        "crop PDF page grid",
    )
    click(".collection-page-card > button")
    until(
        lambda: js(
            "return document.querySelector('.collection-reader img')?.naturalWidth>0"
        ),
        "native PDF crop reader",
    )
    assert not js(
        "return !!document.querySelector('.collection-reader iframe,.collection-reader .collection-text-label')"
    )
    crop_pixels = pdf_pixels(
        js("return document.querySelector('.collection-reader img').src")
    )
    assert (crop_pixels["width"], crop_pixels["height"]) == (100, 200)
    assert (
        crop_pixels["top"] == [255, 0, 0, 255]
        and crop_pixels["bottom"] == [0, 0, 255, 255]
        and crop_pixels["green"] == 0
    )
    screenshot("linux-pdf-crop-reader.png")
    button("닫기")
    click(".collection-back")
    button("PDF 원본 네 페이지")
    until(
        lambda: js(
            "return document.querySelectorAll('.collection-page-card').length===4"
        ),
        "four original PDF pages",
    )
    click(".collection-page-card > button")
    until(
        lambda: js(
            "return document.querySelector('.collection-reader img')?.naturalWidth>0"
        ),
        "mixed PDF first physical page",
    )
    first_pdf_src = js("return document.querySelector('.collection-reader img').src")
    click(".collection-reader-nav button:last-child")
    until(
        lambda: js(
            "const image=document.querySelector('.collection-reader img');return image?.complete&&image.naturalWidth>0&&image.src!==arguments[0]",
            first_pdf_src,
        ),
        "mixed PDF second physical page",
    )
    assert js(
        "return document.querySelector('.collection-reader-nav span').textContent.includes('2 / 4')"
    )
    scan_pixels = pdf_pixels(
        js("return document.querySelector('.collection-reader img').src")
    )
    assert scan_pixels["ink"] > 1000, scan_pixels
    screenshot("linux-pdf-scan-reader.png")
    button("닫기")
    until(
        lambda: (
            invoke("collection_reading", {"workId": pdf_cases[1]["work"]["id"]}).get(
                "pageId"
            )
            == pdf_cases[1]["pages"][1]["id"]
        ),
        "physical PDF page bookmark",
    )
    records["pdf"][1]["physicalSecondPageBookmark"] = True
    click(".collection-back")
    target = pdf_cases[0]
    target["source"].write_bytes(target["original"] + b"changed owned PDF")
    try:
        invoke(
            "collection_media", {"pageId": target["pages"][0]["id"], "thumbnail": False}
        )
        raise AssertionError("Changed PDF accepted")
    except RuntimeError as error:
        assert json.loads(str(error))["code"] == "sourceChanged", str(error)
    finally:
        target["source"].write_bytes(target["original"])
    records["pdf"][0]["changedOriginalRejected"] = True
    assert all(case["source"].read_bytes() == case["original"] for case in pdf_cases)
    stage = "real literal text reader"
    button("한글 첫 페이지")
    until(
        lambda: js(
            "return document.querySelectorAll('.collection-page-card').length>1"
        ),
        "text pages",
    )
    click(".collection-page-card > button")
    until(
        lambda: js(
            "return document.querySelector('.collection-reader iframe')?.contentDocument?.querySelector('pre')?.textContent.startsWith('제1장')"
        ),
        "literal Korean text reader",
    )
    reader = js(
        "const f=document.querySelector('.collection-reader iframe');return {text:f.contentDocument.querySelector('pre').textContent,scripts:f.contentDocument.querySelectorAll('script,img').length,leaked:f.contentWindow.BAD_TEXT_SCRIPT===true}"
    )
    plain = invoke("collection_media", {"pageId": pages[0]["id"], "thumbnail": False})
    assert (
        reader["text"] == plain["text"]
        and reader["scripts"] == 0
        and not reader["leaked"]
    )
    screenshot("linux-reader.png")
    command("POST", "/window/rect", {"width": 1024, "height": 768})
    screenshot("linux-reader-1024.png")
    records["collection"] = {
        "pages": len(pages),
        "cover": cover["pageId"],
        "literalText": True,
        "pixels": pixels,
    }
    source.write_text(original + "\n변경된 원문")
    try:
        invoke("collection_media", {"pageId": pages[0]["id"], "thumbnail": False})
        raise AssertionError("Changed original was accepted")
    except RuntimeError as error:
        assert json.loads(str(error))["code"] == "sourceChanged", str(error)
    source.write_text(original)
    records["collection"]["changedOriginalRejected"] = True
    errors = {}
    for name, handle in [("main", main), ("quick", quick)]:
        command("POST", "/window", {"handle": handle})
        errors[name] = js("return window.__qaErrors")
    records["console"] = errors
    assert not any(errors.values()), errors
    command("POST", "/window", {"handle": main})
    assert not invoke("get_settings")["clipboard"]["enabled"]
    records["passed"] = True
except BaseException as error:
    records["passed"] = False
    records["error"] = repr(error)
    records["stage"] = stage
    if session:
        try:
            records["dom"] = js("return document.body.innerText")[:12000]
            records["windowState"] = js(
                "return {visibility:document.visibilityState,focus:document.hasFocus(),width:innerWidth,height:innerHeight,query:document.querySelector('input')?.value,expanded:!!document.querySelector('.quick-panel.expanded'),active:document.activeElement?.tagName}"
            )
            records["nativeVisible"] = invoke(
                "plugin:window|is_visible",
                {"label": "quick" if stage.startswith("real Spotlight") else "main"},
            )
        except (OSError, RuntimeError, ValueError, AssertionError, KeyError) as nested:
            records["stateDiagnosticError"] = repr(nested)
        try:
            screenshot("linux-failure.png")
        except (OSError, RuntimeError, ValueError, AssertionError, KeyError) as nested:
            records["diagnosticError"] = repr(nested)
    raise
finally:
    if session:
        try:
            js("void window.__TAURI_INTERNALS__.invoke('quit_app'); return true")
        except (OSError, RuntimeError, ValueError) as error:
            records["quitDiagnostic"] = repr(error)
        try:
            http("DELETE", "/session/" + session)
        except (OSError, RuntimeError, ValueError) as error:
            records["sessionDeleteDiagnostic"] = repr(error)
    for child in (driver, wm, xvfb):
        if child and child.poll() is None:
            # Only the fresh group this harness created, including its driver
            # descendants; no executable-name/process-inventory termination.
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(10)
    if runtime_temp_identity:
        try:
            # This optional directory was freshly created by this invocation.
            # Retain the profile and original AppImage; remove only runtime
            # extraction/cache bytes after shutting down the owned desktop.
            assert temporary.resolve() == temporary
            assert temporary.parent == args.runtime_temp_parent.resolve(strict=True)
            assert temporary.name.startswith("prism-linux-runtime-")
            metadata = temporary.stat()
            assert (metadata.st_dev, metadata.st_ino) == runtime_temp_identity
            assert (
                temporary / "qa-owner.txt"
            ).read_bytes() == b"Prism native Linux runtime temp v1\n"
            assert shutil.rmtree.avoids_symlink_attacks
            payloads = []
            for executable in temporary.glob("appimage_extracted_*/usr/bin/prism"):
                assert executable.resolve().is_relative_to(temporary)
                assert executable.is_file() and not executable.is_symlink()
                payloads.append(
                    {
                        "relativePath": executable.relative_to(temporary).as_posix(),
                        "bytes": executable.stat().st_size,
                        "sha256": file_hash(executable),
                    }
                )
            records["runtimePayloads"] = payloads
            shutil.rmtree(temporary)
            records["runtimeTemporaryRemoved"] = not temporary.exists()
        except (OSError, RuntimeError, ValueError, AssertionError) as error:
            records["passed"] = False
            records["runtimeCleanupError"] = repr(error)
    # Check source stability only after owned processes have been stopped.
    unchanged = (
        file_hash(Path(__file__)) == script_sha and file_hash(candidate) == app_sha
    )
    if not unchanged:
        records["passed"] = False
    records.update(
        {
            "appSha256": app_sha,
            "testSha256": script_sha,
            "testAndExecutableUnchanged": unchanged,
            "fixture": str(fixture),
            "profile": str(profile),
            "stage": stage,
            "arguments": {
                key: str(value) if isinstance(value, Path) else value
                for key, value in vars(args).items()
            },
            "cleanup": {
                "driver": driver.returncode if driver else None,
                "windowManager": wm.returncode if wm else None,
                "xvfb": xvfb.returncode if xvfb else None,
            },
        }
    )
    (fixture / "result.json").write_text(
        json.dumps(records, ensure_ascii=False, indent=2)
    )
    print(
        json.dumps(
            {
                "fixture": str(fixture),
                "appSha256": app_sha,
                "passed": records.get("passed"),
                "stage": stage,
            }
        ),
        flush=True,
    )
    assert unchanged
    assert records.get("passed") is True, records.get("error", stage)
