# Prism Windows long-path patch

Upstream: `tempfile` 3.27.0, from the exact crates.io archive selected by the original lockfile. Archive SHA-256: `32497e9a4c7b38532efcdebeef879707aa9f794296a4f0244f6f69e9bc8574bd`. Upstream MIT and Apache-2.0 licenses are retained.

Lance 14.0.2 uses `TempPath::persist` to publish local transaction files. In a portable data directory, a full transaction path can exceed the Windows `MAX_PATH` limit. The upstream Windows implementation passes raw paths to `SetFileAttributesW` and `MoveFileExW`, so an otherwise valid long path fails with OS error 3. The native Lance regression reproduced the failure without an embedding model.

`src/file/imp/windows.rs` now expands long paths to absolute paths and supplies the extended UTF-16 namespace for disk and UNC paths. Short paths and explicitly namespaced paths retain their original representation. No lossy Unicode conversion or registry setting is used. Atomic overwrite/non-overwrite flags are unchanged. Unix and other platform implementations are unchanged.

The local crate's declared Rust minimum is 1.79 because this patch uses `std::path::absolute`; Prism already requires a newer compiler. Tests cover long Unicode persistence, preservation of existing contents on non-overwriting persistence, short paths and long UNC conversion. Prism also tests actual Lance writes in a long portable directory and actual model/vector reopening after moving a portable copy.

API reference: [MoveFileExW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw) and [Windows path length rules](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation). Remove this Cargo patch when the selected upstream tempfile release provides equivalent handling and these regressions pass without it.
