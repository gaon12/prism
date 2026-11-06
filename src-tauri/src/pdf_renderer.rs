//! Linux PDF rasterization in an owned, one-shot pre-GUI process. Documents go
//! over bounded pipes, never command lines, filenames, model APIs or temp files.
//! Process isolation and resource limits are not a filesystem/network sandbox.
use crate::ocr::{error, fitted, Code, Result};
use std::{
    io::{Read, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const FLAG: &str = "--prism-pdf-renderer-v1";
const READY: &[u8; 9] = b"PRISMPDF0";
const REQUEST: &[u8; 9] = b"PRISMPDF1";
const RESPONSE: &[u8; 9] = b"PRISMPNG1";
const MAX_INPUT: usize = 128 << 20;
const MAX_OUTPUT: usize = 16 << 20;
const MAX_PIXELS: u64 = 16_000_000;

fn valid(length: usize, page: u32, dimension: u32) -> bool {
    (1..=MAX_INPUT).contains(&length) && page > 0 && (1..=4096).contains(&dimension)
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // This unreaped Child, not a stored PID or a process-name search.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `launcher` is current_exe in production, with the same pre-GUI dispatch as
/// the QA host. It is never configurable through IPC or document metadata.
pub fn render(
    bytes: &[u8],
    page: u32,
    dimension: u32,
    launcher: &Path,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>> {
    if !valid(bytes.len(), page, dimension) {
        return Err(error(Code::InvalidInput));
    }
    check(cancel, deadline)?;
    let mut input = Vec::new();
    input
        .try_reserve_exact(bytes.len())
        .map_err(|_| error(Code::LimitExceeded))?;
    input.extend_from_slice(bytes);
    check(cancel, deadline)?;
    let mut command = Command::new(launcher);
    command
        .arg(FLAG)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // AppImage's bundled shared libraries may require this existing loader path.
    // Do not inherit unrelated API-key/model/configuration environment variables.
    if let Some(value) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", value);
    }
    let parent = std::process::id() as libc::pid_t;
    // Install before exec as well as in standalone worker dispatch. This closes
    // the fork-to-dispatch race, including when a subreaper would adopt the child.
    // SAFETY: after fork this closure uses only async-signal-safe Linux system
    // calls and a raw-OS-error constructor; no locks, allocations or Rust I/O.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL as libc::c_ulong,
                0,
                0,
                0,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::EPIPE));
            }
            Ok(())
        });
    }
    let mut child = OwnedChild(command.spawn().map_err(|_| error(Code::Unavailable))?);
    let mut stdin = child.0.stdin.take().ok_or_else(|| error(Code::Failed))?;
    let stdout = child.0.stdout.take().ok_or_else(|| error(Code::Failed))?;
    let writer = std::thread::Builder::new()
        .name("prism-pdf-input".into())
        .spawn(move || {
            stdin.write_all(REQUEST)?;
            stdin.write_all(&page.to_be_bytes())?;
            stdin.write_all(&dimension.to_be_bytes())?;
            stdin.write_all(&(input.len() as u64).to_be_bytes())?;
            stdin.write_all(&input)
        })
        .map_err(|_| error(Code::Failed))?;
    let (send, receive) = mpsc::sync_channel(1);
    let reader = match std::thread::Builder::new()
        .name("prism-pdf-output".into())
        .spawn(move || {
            let _ = send.send(read_response(stdout));
        }) {
        Ok(reader) => reader,
        Err(_) => {
            drop(child); // Close pipes before joining a possibly blocked writer.
            let _ = writer.join();
            return Err(error(Code::Failed));
        }
    };
    let mut response = None;
    let result = loop {
        if let Err(cause) = check(cancel, deadline) {
            break Err(cause);
        }
        if response.is_none() {
            match receive.try_recv() {
                Ok(value) => response = Some(value),
                Err(mpsc::TryRecvError::Disconnected) => break Err(error(Code::Failed)),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        match child.0.try_wait() {
            Ok(Some(status)) => {
                if let Some(response) = response.take() {
                    break if status.success() {
                        response
                    } else {
                        Err(error(if status.signal() == Some(libc::SIGXCPU) {
                            Code::LimitExceeded
                        } else {
                            Code::Failed
                        }))
                    };
                }
            }
            Err(_) => break Err(error(Code::Failed)),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // Retain the caller's admission lease until the process is reaped and both
    // bounded pipe workers have actually stopped, including on cancellation.
    drop(child);
    let wrote = writer.join().is_ok_and(|result| result.is_ok());
    let read = reader.join().is_ok();
    check(cancel, deadline)?;
    let output = result?;
    if !wrote || !read {
        return Err(error(Code::Failed));
    }
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&output))
        .with_guessed_format()
        .map_err(|_| error(Code::Failed))?
        .into_dimensions()
        .map_err(|_| error(Code::Failed))?;
    if width == 0
        || height == 0
        || width.max(height) > dimension
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(error(Code::LimitExceeded));
    }
    Ok(output)
}
fn check(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(error(Code::Cancelled))
    } else if Instant::now() >= deadline {
        Err(error(Code::TimedOut))
    } else {
        Ok(())
    }
}
fn read_response(mut source: impl Read) -> Result<Vec<u8>> {
    let mut ready = [0; 9];
    source
        .read_exact(&mut ready)
        .map_err(|_| error(Code::Failed))?;
    if &ready != READY {
        return Err(error(Code::Failed));
    }
    let mut header = [0; 14];
    source
        .read_exact(&mut header)
        .map_err(|_| error(Code::Failed))?;
    let length = u32::from_be_bytes(header[10..14].try_into().unwrap()) as usize;
    if &header[..9] != RESPONSE || length > MAX_OUTPUT || (header[9] != 0 && length != 0) {
        return Err(error(Code::Failed));
    }
    let mut output = vec![0; length];
    source
        .read_exact(&mut output)
        .map_err(|_| error(Code::Failed))?;
    let mut trailing = [0];
    if source
        .read(&mut trailing)
        .map_err(|_| error(Code::Failed))?
        != 0
    {
        return Err(error(Code::Failed));
    }
    match header[9] {
        0 if output.starts_with(b"\x89PNG\r\n\x1a\n") => Ok(output),
        1 => Err(error(Code::InvalidInput)),
        2 => Err(error(Code::CorruptDocument)),
        3 => Err(error(Code::LimitExceeded)),
        _ => Err(error(Code::Failed)),
    }
}

/// Recognize only this exact internal mode; malformed invocations never fall
/// through into a GUI, nor read settings, source paths, credentials or models.
pub fn dispatch() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return None;
    }
    if args.next().is_some() {
        return Some(64);
    }
    Some(if worker().is_ok() { 0 } else { 70 })
}
fn worker_limits() -> std::io::Result<()> {
    // Linux binds this signal to the creating thread. Production callers retain
    // that thread until render returns; normal errors also kill/reap via RAII.
    let parent = unsafe { libc::getppid() };
    if parent <= 1 {
        return Err(std::io::Error::other("Parent unavailable"));
    }
    if unsafe {
        libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::SIGKILL as libc::c_ulong,
            0,
            0,
            0,
        )
    } != 0
        || unsafe { libc::getppid() } != parent
    {
        return Err(std::io::Error::other("Parent unavailable"));
    }
    for (resource, budget) in [
        (libc::RLIMIT_AS, 2u64 << 30),
        (libc::RLIMIT_CPU, 10),
        (libc::RLIMIT_CORE, 0),
        (libc::RLIMIT_NOFILE, 64),
    ] {
        let mut previous = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(resource, &mut previous) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let limits = libc::rlimit {
            rlim_cur: previous.rlim_cur.min(budget),
            rlim_max: previous.rlim_max.min(if resource == libc::RLIMIT_CPU {
                budget + 1 // SIGXCPU at the soft ceiling; hard kill one second later.
            } else {
                budget
            }),
        };
        if unsafe { libc::setrlimit(resource, &limits) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}
fn worker() -> std::io::Result<()> {
    worker_limits()?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(READY)?;
    stdout.flush()?;
    let result = read_request(std::io::stdin().lock())
        .and_then(|(bytes, page, dimension)| pixels(bytes, page, dimension));
    let (status, bytes) = match result {
        Ok(bytes) => (0, bytes),
        Err(cause) => (
            match cause.code {
                Code::InvalidInput => 1,
                Code::CorruptDocument => 2,
                Code::LimitExceeded => 3,
                _ => 4,
            },
            Vec::new(),
        ),
    };
    stdout.write_all(RESPONSE)?;
    stdout.write_all(&[status])?;
    stdout.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stdout.write_all(&bytes)?;
    stdout.flush()
}
fn read_request(mut source: impl Read) -> Result<(Vec<u8>, u32, u32)> {
    let mut header = [0; 25];
    source
        .read_exact(&mut header)
        .map_err(|_| error(Code::InvalidInput))?;
    let page = u32::from_be_bytes(header[9..13].try_into().unwrap());
    let dimension = u32::from_be_bytes(header[13..17].try_into().unwrap());
    let length = u64::from_be_bytes(header[17..25].try_into().unwrap());
    if &header[..9] != REQUEST
        || length > MAX_INPUT as u64
        || !valid(length as usize, page, dimension)
    {
        return Err(error(Code::InvalidInput));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length as usize)
        .map_err(|_| error(Code::LimitExceeded))?;
    bytes.resize(length as usize, 0);
    source
        .read_exact(&mut bytes)
        .map_err(|_| error(Code::InvalidInput))?;
    let mut trailing = [0];
    if source
        .read(&mut trailing)
        .map_err(|_| error(Code::InvalidInput))?
        != 0
    {
        return Err(error(Code::InvalidInput));
    }
    Ok((bytes, page, dimension))
}
fn pixels(bytes: Vec<u8>, page: u32, dimension: u32) -> Result<Vec<u8>> {
    use hayro::{
        hayro_interpret::{util::TransformExt, InterpreterSettings},
        hayro_syntax::Pdf,
        kurbo::Affine,
        vello_cpu::{
            color::palette::css::WHITE, Pixmap, RasterizerSettings, RenderContext, Resources,
            TargetInit,
        },
        RenderCache, RenderSettings,
    };
    let pdf = Pdf::new(bytes).map_err(|_| error(Code::CorruptDocument))?;
    let page = pdf
        .pages()
        .get(page as usize - 1)
        .ok_or_else(|| error(Code::InvalidInput))?;
    let (width, height) = page.render_dimensions();
    let (target_width, target_height) = fitted(
        f64::from(width),
        f64::from(height),
        dimension,
        MAX_PIXELS,
        false,
    )?;
    // Use an exact integer canvas: truncating f32-scaled dimensions can turn
    // an unusually narrow one-pixel edge into a zero-sized render target.
    let mut context = RenderContext::new(target_width as u16, target_height as u16);
    let transform = Affine::scale_non_uniform(
        f64::from(target_width) / f64::from(width),
        f64::from(target_height) / f64::from(height),
    ) * page.initial_transform(true).to_kurbo();
    hayro::render_into(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &mut context,
        transform,
    );
    context.flush();
    let mut pixmap = Pixmap::new(target_width as u16, target_height as u16);
    context.render_with(
        &mut pixmap,
        &mut Resources::default(),
        RasterizerSettings {
            target_init: TargetInit::Clear(WHITE),
            ..Default::default()
        },
    );
    let bytes = pixmap.into_png().map_err(|_| error(Code::Failed))?;
    if bytes.len() > MAX_OUTPUT {
        return Err(error(Code::LimitExceeded));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_pdf_crop_rotation_and_original_pixels() {
        let bytes = include_bytes!("../../tests/fixtures/ocr/crop-clockwise.pdf");
        let output = pixels(bytes.to_vec(), 1, 480).unwrap();
        let image = image::load_from_memory(&output).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (100, 200));
        assert_eq!(image.get_pixel(50, 25).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(50, 175).0, [0, 0, 255, 255]);
        assert!(!image.pixels().any(|p| p[1] > 200 && p[0] < 30 && p[2] < 30));
        assert_eq!(
            pixels(bytes.to_vec(), 2, 480).unwrap_err().code,
            Code::InvalidInput
        );
        assert_eq!(
            pixels(b"damaged".to_vec(), 1, 480).unwrap_err().code,
            Code::CorruptDocument
        );
    }
    #[test]
    fn framing_rejects_oversize_truncated_and_trailing_data_before_parsing() {
        let request = |length: u64, payload: &[u8]| {
            let mut bytes = REQUEST.to_vec();
            bytes.extend_from_slice(&1u32.to_be_bytes());
            bytes.extend_from_slice(&480u32.to_be_bytes());
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(payload);
            bytes
        };
        for input in [
            request(MAX_INPUT as u64 + 1, &[]),
            request(5, b"x"),
            request(1, b"xx"),
            request(0, &[]),
        ] {
            assert_eq!(
                read_request(&input[..]).unwrap_err().code,
                Code::InvalidInput
            );
        }
        assert_eq!(read_request(&request(1, b"x")[..]).unwrap().0, b"x");
        let mut response = READY.to_vec();
        response.extend_from_slice(RESPONSE);
        response.push(0);
        response.extend_from_slice(&(MAX_OUTPUT as u32 + 1).to_be_bytes());
        assert_eq!(read_response(&response[..]).unwrap_err().code, Code::Failed);
        response[18] = 2;
        response[19..23].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(
            read_response(&response[..]).unwrap_err().code,
            Code::CorruptDocument
        );
        response.push(0);
        assert_eq!(read_response(&response[..]).unwrap_err().code, Code::Failed);
    }
}
