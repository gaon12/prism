# Setup core

`npm run test:setup-core` compiles the same install, payload, path, transaction,
argument and Windows integration modules used by the setup program, without its
Tauri/WebView interface. The tests use temporary files and a fake `System`:
they do not register an application, create system shortcuts or launch Prism.
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

An unexpected unwind retains the file map and backups. Automatic recovery after
a killed process or power loss is not implemented yet. Backend concurrency,
the setup UI, actual registry/shortcut behavior, installed updater execution,
restart and MSIX integration remain separate checks. These core tests do not
make the setup program ready to publish.
