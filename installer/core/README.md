# Setup core

`npm run test:setup-core` compiles the same install, payload, path, transaction,
argument and Windows integration modules used by the setup program, without its
Tauri/WebView interface. The tests use temporary files and a fake `System`:
they do not register an application, change the user's Desktop links or launch Prism.
On Windows a real COM shortcut roundtrip is confined to a temporary directory.
The Windows junction regression creates a junction entirely inside a temporary
fixture using a hidden PowerShell process.

Recorded paths must be relative, remain inside the resolved install root and
avoid symbolic links, junctions, Windows device names and alternate data streams.
Shortcut records can select only the known-folder Prism links; changed targets
are preserved. Damaged records stop removal rather than selecting default files.
Payloads are checked for duplicate names and incomplete bodies before extraction.

Setup stages new files and backs up the old app, resources, record, shortcuts and
registration before replacement. Ordinary installation failures restore that
state. Incomplete recovery is reported and retains recovery data. Restoring a
shortcut uses a temporary file beside its destination, allowing the install and
Desktop folders to reside on different drives.

Before replacing installed files, setup flushes a versioned journal and hashes
the file map, original registration and file backups. The next install or removal
recovers an interrupted transaction under the folder lock before reading the live
install record. A completed transaction only needs cleanup. Invalid, ambiguous or
legacy recovery data is preserved and reported instead of replayed. Recovery also
refuses changed shortcuts or registration belonging to another install folder.

Native state guards block overlapping requests and closing during writes; an OS
lock serializes independent processes targeting one folder. Launch authorization
comes from a completed installation rather than a frontend path.

The suite externally terminates its own marked fixture processes at four install
boundaries and three previous-version restore boundaries, then checks recovery,
lock release, user-file preservation and repeat attempts. The ignored fixture
entry is invoked by those tests and by native QA.
These process-termination tests do not establish power-loss or hardware durability.

`npm run test:setup` separately exercises the actual setup UI, installed app launch,
silent updates and both removal modes in a temporary sandbox.
`npm run test:setup-recovery` checks the actual UI against a process-exit journal
and a damaged live record without launching the installed app. Real HKCU writes,
updater-initiated signed installation, power loss, actual older-release data
compatibility and MSIX integration remain separate checks.

Updates to a different version retain a checked previous app archive. Manual
restoration uses a streamed file source through the same install engine instead
of assembling a large in-memory payload. The archive and current file hashes are
checked before mutation, and selected versions are revalidated under the lock.
Reserved setup paths cannot be named by the installation's owned-file record.
Restore failure and process termination return to the current app unless the
restore transaction already committed. User data is never rolled back.
