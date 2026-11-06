"""One owned LibreOffice conversion; binary documents never become CLI paths.

This worker is not connected to the app yet. See README.md for the parent-side
verification, deadline, byte-budget and cancellation requirements.
"""

import ctypes
import io
import json
import os
import subprocess
import sys
import threading
import time
import uuid
from contextlib import suppress
from pathlib import Path

MAX_BYTES = 64 << 20
OWNER = b"Prism Office preview worker v1\n"
FILTERS = {
    "docx": ("Office Open XML Text", "writer_pdf_Export"),
    "xlsx": ("Calc MS Excel 2007 XML", "calc_pdf_Export"),
    "pptx": ("Impress MS PowerPoint 2007 XML", "impress_pdf_Export"),
}


class WorkerError(Exception):
    def __init__(self, code):
        self.code = code


def read_exact(stream, count):
    chunks = bytearray()
    while len(chunks) < count:
        chunk = stream.read(min(65536, count - len(chunks)))
        if not chunk:
            raise WorkerError("protocol")
        chunks.extend(chunk)
    return bytes(chunks)


def read_request(stream):
    # Use the same unbuffered stream for framing and the parent lifeline. No
    # buffered prefetch may hide a cancellation byte behind the document body.
    header = bytearray()
    while len(header) <= 4096:
        value = stream.read(1)
        if not value:
            raise WorkerError("protocol")
        header.extend(value)
        if value == b"\n":
            break
    if len(header) > 4096 or not header.endswith(b"\n"):
        raise WorkerError("protocol")
    try:
        request = json.loads(header)
    except (ValueError, UnicodeError):
        raise WorkerError("protocol") from None
    if not isinstance(request, dict) or set(request) != {
        "version",
        "bytes",
        "extension",
    }:
        raise WorkerError("protocol")
    if type(request["version"]) is not int or request["version"] != 1:
        raise WorkerError("protocol")
    size, extension = request["bytes"], request["extension"]
    if (
        type(size) is not int
        or not isinstance(extension, str)
        or extension not in FILTERS
    ):
        raise WorkerError("input")
    if not 0 < size <= MAX_BYTES:
        raise WorkerError("limit")
    return extension, read_exact(stream, size)


def work_directory(path):
    path = Path(path).absolute()
    # Check before resolving so an ancestor junction cannot disappear from the
    # spelling we validate. The parent must already own this fresh private dir.
    for component in (path, *path.parents):
        if component.is_symlink() or getattr(component, "is_junction", lambda: False)():
            raise WorkerError("input")
    marker = path / "owner.txt"
    if (
        path.parent == path
        or not path.is_dir()
        or marker.is_symlink()
        or not marker.is_file()
        or marker.stat().st_size != len(OWNER)
        or marker.read_bytes() != OWNER
    ):
        raise WorkerError("input")
    return path.resolve(strict=True)


def parent_lifeline(stream):
    # EOF means the parent disappeared. Any byte means explicit cancellation.
    # os._exit also handles a blocked native UNO call; Windows then closes the
    # sole Job Object handle and ends every engine descendant. Nothing is looked
    # up by process name or by a PID saved after its owning handle was reaped.
    try:
        stream.read(1)
    finally:
        os._exit(72)


def contain_windows_worker():
    # Join before spawning anything. Every later descendant inherits this job,
    # including soffice.bin launched by soffice.com. Only this worker holds its
    # non-inheritable handle; OS process teardown closes it even after a kill.
    from ctypes import wintypes as w

    class Basic(ctypes.Structure):
        _fields_ = [
            ("user", ctypes.c_longlong),
            ("jobuser", ctypes.c_longlong),
            ("flags", w.DWORD),
            ("minws", ctypes.c_size_t),
            ("maxws", ctypes.c_size_t),
            ("active", w.DWORD),
            ("affinity", ctypes.c_size_t),
            ("priority", w.DWORD),
            ("scheduling", w.DWORD),
        ]

    class Io(ctypes.Structure):
        _fields_ = [
            (name, ctypes.c_ulonglong)
            for name in (
                "read",
                "write",
                "other",
                "readbytes",
                "writebytes",
                "otherbytes",
            )
        ]

    class Limits(ctypes.Structure):
        _fields_ = [
            ("basic", Basic),
            ("io", Io),
            ("processmemory", ctypes.c_size_t),
            ("jobmemory", ctypes.c_size_t),
            ("peakprocess", ctypes.c_size_t),
            ("peakjob", ctypes.c_size_t),
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, w.LPCWSTR]
    kernel.CreateJobObjectW.restype = w.HANDLE
    kernel.SetInformationJobObject.argtypes = [
        w.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        w.DWORD,
    ]
    kernel.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]
    kernel.GetCurrentProcess.restype = w.HANDLE
    kernel.CloseHandle.argtypes = [w.HANDLE]
    handle = kernel.CreateJobObjectW(None, None)
    limits = Limits()
    limits.basic.flags = 0x2000 | 0x200  # kill-on-close and whole-job memory bound
    limits.jobmemory = 2 << 30
    if not handle or not kernel.SetInformationJobObject(
        handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)
    ):
        if handle:
            kernel.CloseHandle(handle)
        raise WorkerError("unavailable")
    if not kernel.AssignProcessToJobObject(handle, kernel.GetCurrentProcess()):
        kernel.CloseHandle(handle)
        raise WorkerError("unavailable")
    # Intentionally retained until process teardown, not closed while we ourselves
    # are still in the job. No handle is exposed to or inherited by descendants.
    return handle


def run():
    if sys.platform != "win32":
        # Unix needs the application-owned group supervisor before integration;
        # do not silently run without that lifetime boundary.
        raise WorkerError("unsupported")
    if len(sys.argv) != 3:
        raise WorkerError("input")
    runtime = Path(sys.argv[1]).resolve(strict=True)
    profile = work_directory(sys.argv[2])
    stream = sys.stdin.buffer.raw
    extension, source = read_request(stream)
    size = len(source)
    if source.startswith(bytes.fromhex("d0cf11e0a1b11ae1")):
        raise WorkerError("encrypted")
    _job = contain_windows_worker()
    threading.Thread(target=parent_lifeline, args=(stream,), daemon=True).start()
    _dll_directory = os.add_dll_directory(str(runtime / "program"))
    sys.dont_write_bytecode = True
    os.environ["URE_BOOTSTRAP"] = (runtime / "program" / "fundamental.ini").as_uri()
    os.environ["UNO_PATH"] = str(runtime / "program")
    sys.path.insert(0, str(runtime / "program"))
    import uno
    import unohelper
    from com.sun.star.connection import NoConnectException
    from com.sun.star.io import XInputStream, XOutputStream, XSeekable
    from com.sun.star.task import XInteractionHandler2
    from com.sun.star.uno import Exception as UnoException

    class Input(unohelper.Base, XInputStream, XSeekable):
        def __init__(self):
            self.stream = io.BytesIO(source)

        def readBytes(self, _data, count):
            data = self.stream.read(count)
            return len(data), uno.ByteSequence(data)

        def readSomeBytes(self, data, count):
            return self.readBytes(data, count)

        def skipBytes(self, count):
            self.seek(min(size, self.stream.tell() + count))

        def available(self):
            return size - self.stream.tell()

        def closeInput(self):
            pass

        def seek(self, position):
            if not 0 <= position <= size:
                raise RuntimeError("seek")
            self.stream.seek(position)

        def getPosition(self):
            return self.stream.tell()

        def getLength(self):
            return size

    class Output(unohelper.Base, XOutputStream):
        def __init__(self):
            self.data = bytearray()
            self.over_limit = False

        def writeBytes(self, data):
            value = data.value if hasattr(data, "value") else bytes(data)
            if len(self.data) + len(value) > MAX_BYTES:
                self.over_limit = True
                raise WorkerError("limit")
            self.data.extend(value)

        def flush(self):
            pass

        def closeOutput(self):
            pass

    class Interactions(unohelper.Base, XInteractionHandler2):
        def __init__(self):
            self.count = 0

        def handle(self, request):
            self.handleInteractionRequest(request)

        def handleInteractionRequest(self, request):
            self.count += 1
            for continuation in request.getContinuations():
                abort = continuation.queryInterface(
                    uno.getTypeByName("com.sun.star.task.XInteractionAbort")
                )
                if abort:
                    abort.select()
                    return True
            return False

    def prop(name, value):
        result = uno.createUnoStruct("com.sun.star.beans.PropertyValue")
        result.Name, result.Value = name, value
        return result

    temporary = profile / "Temp"
    temporary.mkdir(exist_ok=True)
    environment = dict(
        os.environ,
        TMP=str(temporary),
        TEMP=str(temporary),
        TMPDIR=str(temporary),
        SAL_DISABLE_OPENCL="1",
        SAL_USE_VCLPLUGIN="svp",
    )
    pipe = "prism-office-" + uuid.uuid4().hex
    started = time.monotonic()
    engine = subprocess.Popen(
        [
            str(runtime / "program" / "soffice.com"),
            "--headless",
            "--nologo",
            "--nodefault",
            "--nofirststartwizard",
            "--norestore",
            "-env:UserInstallation=" + (profile / "user").as_uri(),
            "--accept=pipe,name=" + pipe + ";urp;StarOffice.ComponentContext",
        ],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        creationflags=0x08000000 if os.name == "nt" else 0,
    )
    desktop, document = None, None
    try:
        local = uno.getComponentContext()
        resolver = local.ServiceManager.createInstanceWithContext(
            "com.sun.star.bridge.UnoUrlResolver", local
        )
        deadline = time.monotonic() + 15
        while True:
            try:
                context = resolver.resolve(
                    "uno:pipe,name=" + pipe + ";urp;StarOffice.ComponentContext"
                )
                break
            except NoConnectException:
                if time.monotonic() >= deadline or engine.poll() is not None:
                    raise WorkerError("unavailable")
                time.sleep(0.05)
        desktop = context.ServiceManager.createInstanceWithContext(
            "com.sun.star.frame.Desktop", context
        )
        interactions = Interactions()
        document = desktop.loadComponentFromURL(
            "private:stream",
            "_blank",
            0,
            (
                prop("InputStream", Input()),
                prop("FilterName", FILTERS[extension][0]),
                prop("Hidden", True),
                prop("ReadOnly", True),
                prop("Silent", True),
                prop("MacroExecutionMode", uno.Any("short", 0)),
                prop("UpdateDocMode", uno.Any("short", 0)),
                prop("PickListEntry", False),
                prop("InteractionHandler", interactions),
            ),
        )
        if document is None:
            raise WorkerError("unsupported")
        output = Output()
        try:
            document.storeToURL(
                "private:stream",
                (
                    prop("FilterName", FILTERS[extension][1]),
                    prop("OutputStream", output),
                ),
            )
        except UnoException:
            raise WorkerError("limit" if output.over_limit else "unsupported") from None
        if not output.data.startswith(b"%PDF-"):
            raise RuntimeError("output")
        result = {
            "version": 1,
            "ok": True,
            "pdfBytes": len(output.data),
            "interactions": interactions.count,
            "elapsedMs": round((time.monotonic() - started) * 1000),
        }
        sys.stdout.buffer.write(json.dumps(result).encode() + b"\n" + output.data)
        sys.stdout.buffer.flush()
    finally:
        if document is not None:
            with suppress(UnoException):
                document.close(True)
        if desktop is not None:
            with suppress(UnoException):
                desktop.terminate()
        try:
            engine.wait(timeout=5)
        except subprocess.TimeoutExpired:
            engine.kill()
            engine.wait()


if __name__ == "__main__":
    try:
        run()
    except Exception as error:  # noqa: BLE001 -- final privacy boundary, fixed tags only
        # No exception text or source data may cross this diagnostic boundary.
        code = error.code if isinstance(error, WorkerError) else "failed"
        sys.stdout.buffer.write(
            json.dumps({"version": 1, "ok": False, "code": code}).encode() + b"\n"
        )
        sys.stdout.buffer.flush()
        raise SystemExit(2)
