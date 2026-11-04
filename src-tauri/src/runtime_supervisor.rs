//! A Unix-only, pre-GUI execution mode. The parent holds the only write end of
//! stdin; EOF means it has stopped or died. Only this process's own Child is
//! killed and reaped. No shell, pid-file lookup, document text or API key argv.
#[cfg(unix)]
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

#[cfg(unix)]
pub const FLAG: &str = "--prism-runtime-supervisor-v1";

/// Call before initializing Tauri, single-instance registration or any UI.
/// A supervisor does not read application settings or create a desktop window.
pub fn dispatch() -> Option<i32> {
    #[cfg(unix)]
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
            return None;
        }
        Some(match request(args.collect()) {
            Ok((program, args)) => serve(program, args).unwrap_or(70),
            Err(_) => 64,
        })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn request(mut args: Vec<OsString>) -> io::Result<(OsString, Vec<OsString>)> {
    if args.len() < 2
        || args.len() > 130
        || args[1] != "--"
        || !Path::new(&args[0]).is_absolute()
        || args
            .iter()
            .map(|arg| arg.as_encoded_bytes().len())
            .sum::<usize>()
            > 65536
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid runtime supervisor request",
        ));
    }
    let remaining = args.split_off(2);
    Ok((args.remove(0), remaining))
}

#[cfg(unix)]
pub(super) fn spawn(
    command: Command,
    launcher: &Path,
) -> io::Result<crate::runtime_process::OwnedRuntimeProcess> {
    use std::os::unix::process::CommandExt;
    let mut owner = Command::new(launcher);
    owner
        .arg(FLAG)
        .arg(command.get_program())
        .arg("--")
        .args(command.get_args());
    if let Some(directory) = command.get_current_dir() {
        owner.current_dir(directory);
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                owner.env(key, value);
            }
            None => {
                owner.env_remove(key);
            }
        }
    }
    // Terminal signals addressed to the app's process group must not also kill
    // the supervisor before it can handle stdin EOF and reap the server.
    owner
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut owned = crate::runtime_process::OwnedRuntimeProcess::supervised(owner.spawn()?)?;
    owned.handshake()?;
    Ok(owned)
}

#[cfg(unix)]
struct Server(Child);
#[cfg(unix)]
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn serve(program: OsString, args: Vec<OsString>) -> io::Result<i32> {
    let (sender, closed) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("prism-runtime-parent".into())
        .spawn(move || {
            let mut byte = [0];
            loop {
                match io::stdin().read(&mut byte) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    _ => break,
                }
            }
            let _ = sender.send(());
        })?;
    let mut server = Server(
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    if closed.try_recv().is_ok() {
        return Ok(0);
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"PRISMSUP1")?;
    stdout.write_all(&server.0.id().to_be_bytes())?;
    stdout.flush()?;
    drop(stdout);
    loop {
        if let Some(status) = server.0.try_wait()? {
            return Ok(status.code().unwrap_or(1));
        }
        match closed.recv_timeout(Duration::from_millis(50)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => return Ok(0), // Server's Drop kills and reaps only our child.
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn request_is_literal_bounded_and_requires_an_absolute_program() {
        let (_, args) = request(vec![
            "/tmp/a runtime".into(),
            "--".into(),
            "$(do-not-evaluate)".into(),
            "a;b".into(),
        ])
        .unwrap();
        assert_eq!(args, ["$(do-not-evaluate)", "a;b"]);
        for bad in [
            vec![],
            vec!["/bin/true".into()],
            vec!["relative".into(), "--".into()],
            vec!["/bin/true".into(), "wrong".into()],
            vec!["/bin/true".into(), "--".into(), "x".repeat(65537).into()],
        ] {
            assert!(request(bad).is_err());
        }
        assert!(request(
            std::iter::once("/bin/true".into())
                .chain(std::iter::once("--".into()))
                .chain(std::iter::repeat_n("a".into(), 129))
                .collect()
        )
        .is_err());
    }
}
