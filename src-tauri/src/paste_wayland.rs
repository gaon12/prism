//! Paste into other apps on a Wayland desktop.
//!
//! Wayland does not let an app read which window is in front or press keys for another app.
//! So this works differently from Windows and X11:
//!
//! - The "target" is simply whatever app the desktop gives focus to after Prism hides. When
//!   Prism opens from a shortcut and then hides again, the desktop returns focus to the app
//!   that was in front before, which is the app the person was working in.
//! - Keys are sent through the desktop's own "remote desktop" permission portal
//!   (`org.freedesktop.portal.RemoteDesktop`, available on GNOME and KDE). The first paste in a
//!   Prism session shows the desktop's permission dialog; the approved session is then kept
//!   for the rest of the run, so later pastes need no dialog.
//! - Desktops without that portal (for example Sway or Hyprland) can use the `wtype` tool if it
//!   is installed. It is run directly (no shell) with fixed arguments and a short timeout.
//!
//! If neither is available, or the person declines the dialog, the copy is kept and the paste
//! reports blocked input, exactly like a refused paste on other platforms. No window titles,
//! app names or clipboard contents are read or logged here.
use super::WindowId;
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

/// The one remembered "target": the app that gets focus back after Prism hides.
/// Any value other than 0 works; `paste_target` only compares it with itself.
pub(super) const TARGET: WindowId = 1;
/// Paired with `TARGET`. Not a real process id: Wayland does not tell apps who owns the
/// focused window. It must not be 0 or Prism's own id, which `paste_target` rejects.
pub(super) const TARGET_PID: u32 = u32::MAX;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REMOTE_DESKTOP: &str = "org.freedesktop.portal.RemoteDesktop";
const REQUEST: &str = "org.freedesktop.portal.Request";
/// `SelectDevices` device type bit for a keyboard.
const KEYBOARD: u32 = 1;
/// X keysyms the portal expects: left Control and lowercase v.
const CONTROL_L: i32 = 0xffe3;
const LOWER_V: i32 = 0x0076;
/// Key states for `NotifyKeyboardKeysym`.
const RELEASED: u32 = 0;
const PRESSED: u32 = 1;

/// How long to look for the portal before deciding paste is unavailable.
const PROBE_WAIT: Duration = Duration::from_millis(500);
/// The first paste may wait for the person to answer the desktop's permission dialog.
const APPROVAL_WAIT: Duration = Duration::from_secs(60);
/// Once approved, sending the keys is quick.
const SEND_WAIT: Duration = Duration::from_secs(2);
/// Give the desktop time to move focus back to the previous app after Prism hides.
const FOCUS_SETTLE: Duration = Duration::from_millis(200);
const TOOL_TIMEOUT: Duration = Duration::from_secs(2);
/// Fixed install locations only; nothing is looked up through `PATH`.
const WTYPE: [&str; 2] = ["/usr/bin/wtype", "/usr/local/bin/wtype"];

/// True in a Wayland desktop session, where X11 cannot see or reach most windows.
pub(super) fn session() -> bool {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    set("WAYLAND_DISPLAY")
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"))
}

/// Which way keys can be sent on this desktop, decided once per run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Portal,
    Wtype,
    None,
}

fn route() -> Route {
    static ROUTE: OnceLock<Route> = OnceLock::new();
    *ROUTE.get_or_init(|| {
        if portal_available() {
            Route::Portal
        } else if wtype().is_some() {
            Route::Wtype
        } else {
            Route::None
        }
    })
}

fn wtype() -> Option<&'static str> {
    WTYPE.into_iter().find(|path| Path::new(path).is_file())
}

pub(super) fn supported() -> bool {
    route() != Route::None
}
pub(super) fn foreground() -> Option<WindowId> {
    Some(TARGET)
}
pub(super) fn window_pid(window: WindowId) -> Option<u32> {
    (window == TARGET).then_some(TARGET_PID)
}
/// Prism has already hidden its windows; the desktop moves focus by itself. Just wait a moment
/// for that to happen before keys are sent.
pub(super) fn focus(window: WindowId) -> bool {
    if window != TARGET {
        return false;
    }
    std::thread::sleep(FOCUS_SETTLE);
    true
}
/// Wayland does not let apps read the keyboard state, so this cannot check for held keys.
pub(super) fn keys_released() -> bool {
    true
}
pub(super) fn paste(window: WindowId, pid: u32) -> bool {
    if window != TARGET || pid != TARGET_PID {
        return false;
    }
    match route() {
        Route::Portal => portal().is_some_and(|portal| portal.paste()),
        Route::Wtype => wtype().is_some_and(run_wtype),
        Route::None => false,
    }
}
pub(super) fn shutdown() {
    if let Some(portal) = PORTAL_WORKER.get().and_then(Option::as_ref) {
        portal.stop();
    }
}

/// `wtype -M ctrl v -m ctrl`: hold Control, type v, release Control.
fn run_wtype(program: &str) -> bool {
    super::run_tool(program, &wtype_args(), TOOL_TIMEOUT).is_some()
}
fn wtype_args() -> [&'static str; 5] {
    ["-M", "ctrl", "v", "-m", "ctrl"]
}

/// Asks the session bus whether the desktop offers the remote-desktop portal (version 1 or
/// newer). Runs on its own thread so a stuck bus can never delay opening Prism.
fn portal_available() -> bool {
    let (done, result) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("prism-wayland-probe".into())
        .spawn(move || {
            let found = (|| {
                let connection = Connection::session().ok()?;
                let proxy = Proxy::new(&connection, PORTAL, PORTAL_PATH, REMOTE_DESKTOP).ok()?;
                proxy.get_property::<u32>("version").ok()
            })()
            .is_some_and(|version| version >= 1);
            let _ = done.send(found);
        });
    spawned.is_ok() && result.recv_timeout(PROBE_WAIT).unwrap_or(false)
}

/// One long-lived worker owns the bus connection and the approved portal session. Closing the
/// connection would end the session, so it stays open for the rest of the run.
struct PortalWorker {
    requests: mpsc::SyncSender<(Instant, mpsc::SyncSender<bool>)>,
    stopped: Arc<AtomicBool>,
    approved: Arc<AtomicBool>,
}
static PORTAL_WORKER: OnceLock<Option<PortalWorker>> = OnceLock::new();

fn portal() -> Option<&'static PortalWorker> {
    PORTAL_WORKER.get_or_init(PortalWorker::start).as_ref()
}

impl PortalWorker {
    fn start() -> Option<Self> {
        let (requests, queue) = mpsc::sync_channel::<(Instant, mpsc::SyncSender<bool>)>(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let approved = Arc::new(AtomicBool::new(false));
        let (worker_stopped, worker_approved) = (stopped.clone(), approved.clone());
        std::thread::Builder::new()
            .name("prism-wayland-paste".into())
            .spawn(move || {
                let mut session: Option<Session> = None;
                for (deadline, reply) in queue {
                    let active =
                        || !worker_stopped.load(Ordering::Acquire) && Instant::now() < deadline;
                    if session.is_none() && active() {
                        session = Session::open(&active);
                        worker_approved.store(session.is_some(), Ordering::Release);
                    }
                    // Never press keys for a request whose caller already gave up: a late
                    // approval must not paste into whatever happens to be in front by then.
                    let sent = match session.as_ref() {
                        Some(open) if active() => open.press_ctrl_v(),
                        _ => false,
                    };
                    if !sent {
                        // A failed send usually means the session ended (revoked by the
                        // person, or the desktop restarted). Ask again next time.
                        session = None;
                        worker_approved.store(false, Ordering::Release);
                    }
                    let _ = reply.send(sent);
                }
            })
            .ok()?;
        Some(Self {
            requests,
            stopped,
            approved,
        })
    }
    fn paste(&self) -> bool {
        let wait = if self.approved.load(Ordering::Acquire) {
            SEND_WAIT
        } else {
            APPROVAL_WAIT
        };
        let (reply, answer) = mpsc::sync_channel(1);
        if self
            .requests
            .try_send((Instant::now() + wait, reply))
            .is_err()
        {
            return false;
        }
        answer.recv_timeout(wait).unwrap_or(false)
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}

/// An approved remote-desktop session that may send keyboard input.
struct Session {
    connection: Connection,
    handle: OwnedObjectPath,
}

impl Session {
    /// Create the session, ask for keyboard access and start it. Starting shows the desktop's
    /// permission dialog; `active` stops waiting once the caller has given up.
    fn open(active: &dyn Fn() -> bool) -> Option<Self> {
        let connection = Connection::session().ok()?;
        let sender = connection.unique_name()?.to_string();
        let remote = Proxy::new(&connection, PORTAL, PORTAL_PATH, REMOTE_DESKTOP).ok()?;

        let created = request(&connection, &sender, "create", active, |token| {
            let options: HashMap<&str, Value> = HashMap::from([
                ("handle_token", Value::from(token)),
                ("session_handle_token", Value::from("prism_paste")),
            ]);
            remote.call_method("CreateSession", &(options,)).map(drop)
        })?;
        let handle = created
            .get("session_handle")
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .and_then(|path| OwnedObjectPath::try_from(path).ok())?;

        request(&connection, &sender, "devices", active, |token| {
            let options: HashMap<&str, Value> = HashMap::from([
                ("handle_token", Value::from(token)),
                ("types", Value::from(KEYBOARD)),
            ]);
            remote
                .call_method("SelectDevices", &(&handle, options))
                .map(drop)
        })?;

        let started = request(&connection, &sender, "start", active, |token| {
            let options: HashMap<&str, Value> =
                HashMap::from([("handle_token", Value::from(token))]);
            remote
                .call_method("Start", &(&handle, "", options))
                .map(drop)
        })?;
        // The desktop may grant fewer devices than asked for.
        let devices = started
            .get("devices")
            .and_then(|value| u32::try_from(value.try_clone().ok()?).ok())
            .unwrap_or(0);
        (devices & KEYBOARD != 0).then_some(Self { connection, handle })
    }

    /// Control down, v down, v up, Control up. Each release is attempted even after a failed
    /// press, so a key is never left held down by Prism.
    fn press_ctrl_v(&self) -> bool {
        let Ok(remote) = Proxy::new(&self.connection, PORTAL, PORTAL_PATH, REMOTE_DESKTOP) else {
            return false;
        };
        let send = |keysym: i32, state: u32| {
            let options: HashMap<&str, Value> = HashMap::new();
            remote
                .call_method(
                    "NotifyKeyboardKeysym",
                    &(&self.handle, options, keysym, state),
                )
                .is_ok()
        };
        let mut all = true;
        for (keysym, state) in key_plan() {
            all &= send(keysym, state);
            if !all && state == PRESSED {
                break;
            }
        }
        if !all {
            // Release both keys so a partial chord cannot leave Control or v pressed.
            send(LOWER_V, RELEASED);
            send(CONTROL_L, RELEASED);
        }
        all
    }
}

/// The key events for one paste, in order.
fn key_plan() -> [(i32, u32); 4] {
    [
        (CONTROL_L, PRESSED),
        (LOWER_V, PRESSED),
        (LOWER_V, RELEASED),
        (CONTROL_L, RELEASED),
    ]
}

/// Where the portal will publish the answer to a request: the caller's bus name without the
/// leading `:` and with `.` replaced by `_`, then the token (see the portal's Request docs).
fn request_path(sender: &str, token: &str) -> String {
    let sender = sender.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// Calls one portal method and waits for its `Response` signal. The listener is set up before
/// the call so a fast answer is never missed. Returns the results on success (response 0).
fn request(
    connection: &Connection,
    sender: &str,
    name: &str,
    active: &dyn Fn() -> bool,
    call: impl FnOnce(&str) -> zbus::Result<()>,
) -> Option<HashMap<String, OwnedValue>> {
    let token = format!("prism_{name}_{}", std::process::id());
    let path = request_path(sender, &token);
    let answer = Proxy::new(connection, PORTAL, path.as_str(), REQUEST).ok()?;
    let mut responses = answer.receive_signal("Response").ok()?;
    call(&token).ok()?;
    if !active() {
        return None;
    }
    let message = responses.next()?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().ok()?;
    (code == 0 && active()).then_some(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_follow_the_portal_naming_rule() {
        assert_eq!(
            request_path(":1.42", "prism_start_7"),
            "/org/freedesktop/portal/desktop/request/1_42/prism_start_7"
        );
    }

    #[test]
    fn the_chord_presses_control_first_and_releases_it_last() {
        assert_eq!(
            key_plan(),
            [
                (CONTROL_L, PRESSED),
                (LOWER_V, PRESSED),
                (LOWER_V, RELEASED),
                (CONTROL_L, RELEASED)
            ]
        );
        assert_eq!(wtype_args(), ["-M", "ctrl", "v", "-m", "ctrl"]);
    }

    #[test]
    fn only_the_single_wayland_target_is_accepted() {
        assert_eq!(window_pid(TARGET), Some(TARGET_PID));
        assert_eq!(window_pid(TARGET + 1), None);
        assert!(!focus(TARGET + 1));
        assert!(!paste(TARGET, TARGET_PID - 1));
        assert!(!paste(TARGET + 1, TARGET_PID));
    }
}
