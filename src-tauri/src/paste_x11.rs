//! Native X11 desktop operations. No shell tools, window titles or input event subscription.
//! Requests expire individually; queued work may never initiate input after its caller leaves.
use super::WindowId;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    cookie::VoidCookie,
    errors::ReplyError,
    protocol::{
        xproto::{self, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

const QUERY_WAIT: Duration = Duration::from_millis(100);
const ACTION_WAIT: Duration = Duration::from_secs(1);
static CLIENT: OnceLock<Option<Client>> = OnceLock::new();

#[derive(Clone, Copy)]
enum Command {
    Supported,
    Foreground,
    Pid(u32),
    Focus(u32),
    Paste(u32, u32),
    KeysReleased,
}
enum Response {
    Bool(bool),
    Window(Option<u32>),
    Pid(Option<u32>),
    Failed,
}
struct Ticket {
    expires: Instant,
    stopped: Arc<AtomicBool>,
}
impl Ticket {
    fn active(&self) -> bool {
        !self.stopped.load(Ordering::Acquire) && Instant::now() < self.expires
    }
}
struct Message {
    command: Command,
    ticket: Ticket,
    response: mpsc::SyncSender<Response>,
}
struct Client {
    requests: mpsc::SyncSender<Message>,
    stopped: Arc<AtomicBool>,
}
impl Client {
    fn start(
        mut handle: impl FnMut(Command, &Ticket) -> Response + Send + 'static,
    ) -> Option<(Self, std::thread::JoinHandle<()>)> {
        let (requests, pending) = mpsc::sync_channel::<Message>(1);
        let worker = std::thread::Builder::new()
            .name("prism-x11-paste".into())
            .spawn(move || {
                for message in pending {
                    if !message.ticket.active() {
                        continue;
                    }
                    let result = handle(message.command, &message.ticket);
                    if message.ticket.active() {
                        let _ = message.response.send(result);
                    }
                }
            })
            .ok()?;
        Some((
            Self {
                requests,
                stopped: Arc::new(AtomicBool::new(false)),
            },
            worker,
        ))
    }
    fn call(&self, command: Command, timeout: Duration) -> Option<Response> {
        if self.stopped.load(Ordering::Acquire) {
            return None;
        }
        let (response, result) = mpsc::sync_channel(1);
        let message = Message {
            command,
            ticket: Ticket {
                expires: Instant::now() + timeout,
                stopped: self.stopped.clone(),
            },
            response,
        };
        self.requests.try_send(message).ok()?;
        result.recv_timeout(timeout).ok()
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}
fn client() -> Option<&'static Client> {
    CLIENT
        .get_or_init(|| {
            if !std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
                return None;
            }
            let mut desktop: Option<Desktop> = None;
            Client::start(move |command, ticket| {
                if desktop.is_none() {
                    desktop = Desktop::connect().ok();
                }
                let Some(current) = desktop.as_mut() else {
                    return Response::Failed;
                };
                if !ticket.active() {
                    return Response::Failed;
                }
                match current.run(command, ticket) {
                    Ok(response) => response,
                    Err(()) => {
                        desktop = None;
                        Response::Failed
                    }
                }
            })
            .map(|(client, _worker)| client)
        })
        .as_ref()
}
fn call(command: Command, timeout: Duration) -> Option<Response> {
    client()?.call(command, timeout)
}
pub(super) fn supported() -> bool {
    matches!(
        call(Command::Supported, QUERY_WAIT),
        Some(Response::Bool(true))
    )
}
pub(super) fn foreground() -> Option<WindowId> {
    match call(Command::Foreground, QUERY_WAIT)? {
        Response::Window(Some(window)) => WindowId::try_from(window).ok(),
        _ => None,
    }
}
pub(super) fn window_pid(window: WindowId) -> Option<u32> {
    match call(Command::Pid(u32::try_from(window).ok()?), QUERY_WAIT)? {
        Response::Pid(pid) => pid,
        _ => None,
    }
}
pub(super) fn focus(window: WindowId) -> bool {
    u32::try_from(window).ok().is_some_and(|window| {
        matches!(
            call(Command::Focus(window), ACTION_WAIT),
            Some(Response::Bool(true))
        )
    })
}
pub(super) fn keys_released() -> bool {
    matches!(
        call(Command::KeysReleased, QUERY_WAIT),
        Some(Response::Bool(true))
    )
}
pub(super) fn paste(window: WindowId, pid: u32) -> bool {
    u32::try_from(window).ok().is_some_and(|window| {
        matches!(
            call(Command::Paste(window, pid), ACTION_WAIT),
            Some(Response::Bool(true))
        )
    })
}
pub(super) fn shutdown() {
    if let Some(Some(client)) = CLIENT.get() {
        client.stop();
    }
}

struct Desktop {
    connection: RustConnection,
    root: u32,
    active_atom: u32,
    pid_atom: u32,
}
impl Desktop {
    fn connect() -> Result<Self, ()> {
        let (connection, screen) = x11rb::connect(None).map_err(|_| ())?;
        let version = connection
            .xtest_get_version(2, 2)
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?;
        if version.major_version < 2 {
            return Err(());
        }
        let root = connection.setup().roots.get(screen).ok_or(())?.root;
        let active_atom = connection
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?
            .atom;
        let pid_atom = connection
            .intern_atom(false, b"_NET_WM_PID")
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?
            .atom;
        Ok(Self {
            connection,
            root,
            active_atom,
            pid_atom,
        })
    }
    fn property(&self, window: u32, property: u32, kind: AtomEnum) -> Result<Option<u32>, ()> {
        let value = self
            .connection
            .get_property(false, window, property, kind, 0, 1)
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?;
        if value.type_ != u32::from(kind)
            || value.format != 32
            || value.value_len != 1
            || value.bytes_after != 0
        {
            return Ok(None);
        }
        Ok(value
            .value32()
            .and_then(|mut values| values.next())
            .filter(|value| *value != 0))
    }
    fn foreground(&self) -> Result<Option<u32>, ()> {
        self.property(self.root, self.active_atom, AtomEnum::WINDOW)
    }
    fn pid(&self, window: u32) -> Result<Option<u32>, ()> {
        self.connection
            .get_window_attributes(window)
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?;
        self.property(window, self.pid_atom, AtomEnum::CARDINAL)
    }
    fn keys_released(&self) -> Result<bool, ()> {
        Ok(self
            .connection
            .query_keymap()
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?
            .keys
            .iter()
            .all(|byte| *byte == 0))
    }
    fn run(&mut self, command: Command, ticket: &Ticket) -> Result<Response, ()> {
        match command {
            Command::Supported => Ok(Response::Bool(true)),
            Command::Foreground => self.foreground().map(Response::Window),
            Command::Pid(window) => self.pid(window).map(Response::Pid),
            Command::KeysReleased => self.keys_released().map(Response::Bool),
            Command::Focus(window) => {
                if self.pid(window)?.is_none() || !ticket.active() {
                    return Ok(Response::Bool(false));
                }
                self.connection
                    .map_window(window)
                    .map_err(|_| ())?
                    .check()
                    .map_err(|_| ())?;
                if !ticket.active() {
                    return Ok(Response::Bool(false));
                }
                let event = ClientMessageEvent::new(
                    32,
                    window,
                    self.active_atom,
                    [2, x11rb::CURRENT_TIME, 0, 0, 0],
                );
                self.connection
                    .send_event(
                        false,
                        self.root,
                        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                        event,
                    )
                    .map_err(|_| ())?
                    .check()
                    .map_err(|_| ())?;
                Ok(Response::Bool(true))
            }
            Command::Paste(window, pid) => {
                let setup = self.connection.setup();
                let first = setup.min_keycode.max(8);
                let count = setup
                    .max_keycode
                    .checked_sub(first)
                    .and_then(|count| count.checked_add(1))
                    .ok_or(())?;
                let mapping = self
                    .connection
                    .get_keyboard_mapping(first, count)
                    .map_err(|_| ())?
                    .reply()
                    .map_err(|_| ())?;
                let Some((control, v)) = chord_keycodes(
                    first,
                    usize::from(mapping.keysyms_per_keycode),
                    &mapping.keysyms,
                ) else {
                    return Ok(Response::Bool(false));
                };
                // Keep a second connection to this same live server before the last
                // target/key checks. It can release our attempted downs if the input
                // connection breaks. Never reconnect cleanup into a replacement server.
                let witness = Witness::new(&self.connection, self.root)?;
                let (cleanup, _) = x11rb::connect(None).map_err(|_| ())?;
                cleanup
                    .xtest_get_version(2, 2)
                    .map_err(|_| ())?
                    .reply()
                    .map_err(|_| ())?;
                if self.foreground()? != Some(window)
                    || self.pid(window)? != Some(pid)
                    || !self.keys_released()?
                    || !ticket.active()
                {
                    return Ok(Response::Bool(false));
                }
                let mut wire = NativeKeys {
                    connection: &self.connection,
                    cleanup,
                    witness,
                    pending: Vec::new(),
                    failed: false,
                    retired: false,
                };
                let delivered = deliver_chord(&mut wire, control, v, || ticket.active());
                if wire.failed {
                    Err(())
                } else {
                    Ok(Response::Bool(delivered))
                }
            }
        }
    }
}

/// Inspect the current map on every paste; do not rewrite the user's keymap.
fn chord_keycodes(first: u8, width: usize, keysyms: &[u32]) -> Option<(u8, u8)> {
    if width == 0 || !keysyms.len().is_multiple_of(width) {
        return None;
    }
    let find = |symbols: &[u32]| {
        keysyms
            .chunks_exact(width)
            .position(|row| row.iter().any(|symbol| symbols.contains(symbol)))
            .and_then(|index| u8::try_from(usize::from(first) + index).ok())
    };
    let control = find(&[0xffe3, 0xffe4])?;
    let v = find(&[u32::from(b'v'), u32::from(b'V')])?;
    (control != v).then_some((control, v))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    code: u8,
    down: bool,
}
trait KeyWire {
    fn queue(&mut self, key: Key) -> bool;
    fn confirm(&mut self) -> bool;
    fn release(&mut self, attempted: &[u8]) -> bool {
        let mut queued = true;
        for &code in attempted.iter().rev() {
            queued &= self.queue(Key { code, down: false });
        }
        self.confirm() && queued
    }
}
struct NativeKeys<'a> {
    connection: &'a RustConnection,
    cleanup: RustConnection,
    witness: Witness<'a>,
    pending: Vec<VoidCookie<'a, RustConnection>>,
    failed: bool,
    retired: bool,
}
/// A private, unmapped 1x1 pixmap proves when the server has finished retiring
/// the input client. Its disappearance orders backup releases after any old input.
struct Witness<'a> {
    connection: &'a RustConnection,
    id: u32,
}
impl<'a> Witness<'a> {
    fn new(connection: &'a RustConnection, root: u32) -> Result<Self, ()> {
        let id = connection.generate_id().map_err(|_| ())?;
        connection
            .create_pixmap(1, id, root, 1, 1)
            .map_err(|_| ())?
            .check()
            .map_err(|_| ())?;
        Ok(Self { connection, id })
    }
}
impl Drop for Witness<'_> {
    fn drop(&mut self) {
        let _ = self.connection.free_pixmap(self.id);
    }
}
impl NativeKeys<'_> {
    fn retire_input(&mut self) -> bool {
        if !self.retired {
            use std::os::fd::AsRawFd;
            self.retired = true;
            // SAFETY: this is the live socket owned by our input connection. No
            // descriptor is closed/reused here; further buffered writes must fail.
            unsafe {
                libc::shutdown(self.connection.stream().as_raw_fd(), libc::SHUT_RDWR);
            }
        }
        let start = Instant::now();
        loop {
            let result = self
                .cleanup
                .get_geometry(self.witness.id)
                .map_err(ReplyError::from)
                .and_then(|cookie| cookie.reply());
            match result {
                Err(ReplyError::X11Error(error)) if error.error_code == xproto::DRAWABLE_ERROR => {
                    return true
                }
                Ok(_) if start.elapsed() < Duration::from_millis(500) => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                _ => return false,
            }
        }
    }
}
impl KeyWire for NativeKeys<'_> {
    fn queue(&mut self, key: Key) -> bool {
        match self.connection.xtest_fake_input(
            if key.down {
                xproto::KEY_PRESS_EVENT
            } else {
                xproto::KEY_RELEASE_EVENT
            },
            key.code,
            x11rb::CURRENT_TIME,
            x11rb::NONE,
            0,
            0,
            0,
        ) {
            Ok(cookie) => {
                self.pending.push(cookie);
                true
            }
            Err(_) => {
                self.failed = true;
                false
            }
        }
    }
    fn confirm(&mut self) -> bool {
        if self.connection.flush().is_err() {
            self.failed = true;
            return false;
        }
        // Queue releases before waiting for acknowledgments; don't strand a down
        // merely because the caller's deadline expires while the server replies.
        let mut complete = true;
        for cookie in self.pending.drain(..) {
            complete &= cookie.check().is_ok();
        }
        self.failed |= !complete;
        complete
    }
    fn release(&mut self, attempted: &[u8]) -> bool {
        if !self.failed {
            // Cancellation can happen with a down still buffered locally. Put its
            // release on the SAME ordered stream before considering a backup client.
            let mut queued = true;
            for &code in attempted.iter().rev() {
                queued &= self.queue(Key { code, down: false });
            }
            if self.confirm() && queued {
                return true;
            }
        }
        self.failed = true;
        // Do not let a future request flush old downs after backup releases. Only
        // a confirmed retired client permits releases on the independent connection.
        if !self.retire_input() {
            return false;
        }
        let mut pending = Vec::new();
        let mut complete = true;
        for &code in attempted.iter().rev() {
            match self.cleanup.xtest_fake_input(
                xproto::KEY_RELEASE_EVENT,
                code,
                x11rb::CURRENT_TIME,
                x11rb::NONE,
                0,
                0,
                0,
            ) {
                Ok(cookie) => pending.push(cookie),
                Err(_) => complete = false,
            }
        }
        complete &= self.cleanup.flush().is_ok();
        for cookie in pending {
            complete &= cookie.check().is_ok();
        }
        complete
    }
}
fn release_attempted(wire: &mut impl KeyWire, attempted: &[u8]) {
    if attempted.is_empty() {
        return;
    }
    // Retry key-ups only. No chord/key-down is ever replayed after uncertainty.
    for _ in 0..2 {
        if wire.release(attempted) {
            break;
        }
    }
}
fn deliver_chord(wire: &mut impl KeyWire, control: u8, v: u8, active: impl Fn() -> bool) -> bool {
    let mut attempted = Vec::with_capacity(2);
    for key in [
        Key {
            code: control,
            down: true,
        },
        Key {
            code: v,
            down: true,
        },
        Key {
            code: v,
            down: false,
        },
        Key {
            code: control,
            down: false,
        },
    ] {
        if key.down {
            if !active() {
                release_attempted(wire, &attempted);
                return false;
            }
            attempted.push(key.code);
        }
        if !wire.queue(key) {
            release_attempted(wire, &attempted);
            return false;
        }
    }
    if wire.confirm() {
        true
    } else {
        release_attempted(wire, &attempted);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn current_keyboard_mapping_is_required_without_mutating_it() {
        assert_eq!(
            chord_keycodes(8, 2, &[0, 0, 0xffe4, 0, 0x56, 0x76]),
            Some((9, 10))
        );
        for (first, width, map) in [
            (8, 0, vec![0xffe3, 0x76]),
            (8, 2, vec![0xffe3]),
            (8, 1, vec![0xffe3, 0]),
            (8, 2, vec![0xffe3, 0x76]),
            (255, 1, vec![0xffe3, 0x76]),
        ] {
            assert_eq!(chord_keycodes(first, width, &map), None);
        }
    }
    #[derive(Default)]
    struct Wire {
        events: Vec<Key>,
        fail_at: Option<usize>,
        confirmations: Vec<bool>,
        confirms: usize,
    }
    impl KeyWire for Wire {
        fn queue(&mut self, key: Key) -> bool {
            self.events.push(key);
            self.fail_at != Some(self.events.len())
        }
        fn confirm(&mut self) -> bool {
            let value = self
                .confirmations
                .get(self.confirms)
                .copied()
                .unwrap_or(true);
            self.confirms += 1;
            value
        }
    }
    #[test]
    fn uncertain_delivery_only_releases_attempted_keys_and_never_replays_the_chord() {
        for fail in 1..=4 {
            let mut wire = Wire {
                fail_at: Some(fail),
                ..Default::default()
            };
            assert!(!deliver_chord(&mut wire, 37, 55, || true));
            assert_eq!(
                wire.events
                    .iter()
                    .filter(|key| key.down && key.code == 37)
                    .count(),
                1
            );
            assert_eq!(
                wire.events
                    .iter()
                    .filter(|key| key.down && key.code == 55)
                    .count(),
                usize::from(fail >= 2)
            );
            assert!(wire.events[fail..].iter().all(|key| !key.down));
            assert_eq!(
                wire.events.last(),
                Some(&Key {
                    code: 37,
                    down: false
                })
            );
        }
        let mut wire = Wire {
            confirmations: vec![false, false, false],
            ..Default::default()
        };
        assert!(!deliver_chord(&mut wire, 37, 55, || true));
        assert_eq!(wire.confirms, 3);
        assert_eq!(wire.events.iter().filter(|key| key.down).count(), 2);
    }
    #[test]
    fn expiry_before_second_down_releases_control_without_starting_paste() {
        let calls = AtomicUsize::new(0);
        let mut wire = Wire::default();
        assert!(!deliver_chord(&mut wire, 37, 55, || calls
            .fetch_add(1, Ordering::SeqCst)
            == 0));
        assert_eq!(
            wire.events,
            [
                Key {
                    code: 37,
                    down: true
                },
                Key {
                    code: 37,
                    down: false
                }
            ]
        );
        let mut wire = Wire::default();
        assert!(!deliver_chord(&mut wire, 37, 55, || false));
        assert!(wire.events.is_empty());
    }
    #[test]
    fn expired_and_shutdown_requests_never_start_late_side_effects() {
        let (entered, waiting) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(0);
        let effects = Arc::new(AtomicUsize::new(0));
        let count = effects.clone();
        let (client, worker) = Client::start(move |command, ticket| {
            if matches!(command, Command::Supported) {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
            }
            if ticket.active() && matches!(command, Command::Paste(..)) {
                count.fetch_add(1, Ordering::SeqCst);
            }
            Response::Bool(true)
        })
        .unwrap();
        assert!(client
            .call(Command::Supported, Duration::from_millis(25))
            .is_none());
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(client
            .call(Command::Paste(1, 2), Duration::from_millis(25))
            .is_none());
        assert!(client
            .call(Command::Paste(1, 2), Duration::from_millis(25))
            .is_none());
        client.stop();
        release.send(()).unwrap();
        assert!(client
            .call(Command::Paste(1, 2), Duration::from_millis(25))
            .is_none());
        drop(client);
        worker.join().unwrap();
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}
