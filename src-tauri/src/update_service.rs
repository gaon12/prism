//! When and how Prism checks for, announces and installs updates.
//!
//! This file holds the *rules*; the actual network/installer work is behind the
//! [`UpdateBackend`] trait so the rules can be tested without the internet:
//! - Installed builds: the backend wraps `tauri-plugin-updater` (signed installers from
//!   `https://github.com/gaon12/prism/releases/latest/download/latest.json`).
//! - Portable / isolated builds: the backend only reports a new version (see
//!   `update_notice.rs`); `can_install()` is false, so the UI offers "open download page".
//!
//! The rules (decided by the user):
//! 1. Automatic checks are on by default and can be turned off in settings.
//! 2. The first check waits 30 seconds after start-up; later checks run at most once a day.
//! 3. Prism never installs on its own. It shows "Update x.y.z available" and the user chooses
//!    "Install now" or "Later".
//! 4. Before installing, background work (indexing, embedding) is stopped and settings are
//!    saved, because the installer closes the app.
//! 5. A failed check is silent (the user may simply be offline); a failed install is shown.
//!
//! Production transport and lifecycle integration live in update_backend and update_runtime.

use serde::Serialize;
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

pub use crate::update_notice::{CHECK_INTERVAL, FIRST_CHECK_DELAY};

/// A newer version that the backend found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    /// Plain-text release notes (may be empty).
    pub notes: String,
}

/// Talks to the update server / installer. Implementations must not log release contents.
pub trait UpdateBackend: Send + Sync {
    /// Looks for a newer version. `Ok(None)` means "up to date".
    fn check(&self) -> Result<Option<AvailableUpdate>, String>;
    /// True when this build can install updates itself (installed builds only).
    fn can_install(&self) -> bool;
    /// Downloads, verifies the signature of, and starts installing `version`.
    /// On success the app usually restarts and this never returns.
    fn install(
        &self,
        version: &str,
        prepare: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String>;
}

/// What the settings window and the notification show. Sent to the UI as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Automatic daily checks (user setting, default on).
    pub auto_check: bool,
    pub checking: bool,
    pub installing: bool,
    /// Set when a newer version was found and not yet installed.
    pub available: Option<AvailableUpdate>,
    /// The user pressed "Later" for this version; do not pop up again for it.
    pub dismissed_version: Option<String>,
    /// False for portable builds: the UI shows "Open download page" instead of "Install".
    pub can_install: bool,
    /// Last finished check (milliseconds since 1970), stored in settings by the caller.
    pub last_check_ms: Option<u64>,
    /// Only install errors are shown. Check errors stay silent (often just offline).
    pub install_error: Option<String>,
    pub check_error: Option<String>,
}

/// Hooks the service calls around installation. The app passes closures that stop the
/// content/embedding workers and flush settings to disk.
pub struct InstallHooks {
    pub before_install: Box<dyn Fn() -> Result<(), String> + Send + Sync>,
    /// Called if installing failed, to restart the work stopped by `before_install`.
    pub after_failed_install: Box<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
pub struct UpdateService {
    backend: Arc<dyn UpdateBackend>,
    status: Arc<Mutex<UpdateStatus>>,
    hooks: Arc<InstallHooks>,
    listener: Arc<dyn Fn(UpdateStatus) + Send + Sync>,
    emit_lock: Arc<Mutex<()>>,
}

fn lock(status: &Mutex<UpdateStatus>) -> MutexGuard<'_, UpdateStatus> {
    // A panic elsewhere must not disable updates; keep using the last status.
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl UpdateService {
    /// `auto_check`, `last_check_ms` and `dismissed_version` come from saved settings.
    pub fn new(
        backend: Arc<dyn UpdateBackend>,
        hooks: InstallHooks,
        auto_check: bool,
        last_check_ms: Option<u64>,
        dismissed_version: Option<String>,
    ) -> Self {
        let status = UpdateStatus {
            auto_check,
            can_install: backend.can_install(),
            last_check_ms,
            dismissed_version,
            ..Default::default()
        };
        Self {
            backend,
            status: Arc::new(Mutex::new(status)),
            hooks: Arc::new(hooks),
            listener: Arc::new(|_| {}),
            emit_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        lock(&self.status).clone()
    }
    pub fn with_listener(
        mut self,
        listener: impl Fn(UpdateStatus) + Send + Sync + 'static,
    ) -> Self {
        self.listener = Arc::new(listener);
        self
    }
    fn emit(&self) {
        // Serialize snapshots and notifications so a delayed older event cannot overwrite
        // newer dismissal/check metadata. Listeners only persist/emit, never reenter us.
        let _guard = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
        (self.listener)(self.status());
    }

    pub fn set_auto_check(&self, enabled: bool) -> UpdateStatus {
        lock(&self.status).auto_check = enabled;
        self.emit();
        self.status()
    }

    /// How long the background loop should sleep before calling [`Self::tick`] again.
    /// `uptime` is the time since start-up and `now_ms` the current clock.
    pub fn next_wait(&self, uptime: Duration, now_ms: u64) -> Duration {
        if uptime < FIRST_CHECK_DELAY {
            return FIRST_CHECK_DELAY - uptime;
        }
        let status = self.status();
        if !status.auto_check {
            return Duration::from_secs(60);
        }
        match status.last_check_ms {
            Some(last) if now_ms >= last => {
                let since = Duration::from_millis(now_ms - last);
                // Wake up a little after the next check is due; at least once a minute so a
                // re-enabled setting or a clock change is noticed.
                CHECK_INTERVAL
                    .saturating_sub(since)
                    .clamp(Duration::from_secs(1), Duration::from_secs(60))
            }
            _ => Duration::from_secs(60),
        }
    }

    /// Runs an automatic check when one is due. Returns true when a *new* notification should
    /// be shown (a newer version that the user has not dismissed).
    pub fn tick(&self, uptime: Duration, now_ms: u64) -> bool {
        let due = {
            let status = self.status();
            uptime >= FIRST_CHECK_DELAY
                && !status.checking
                && !status.installing
                && crate::update_notice::is_due(status.auto_check, status.last_check_ms, now_ms)
        };
        due && self.check_now(now_ms)
    }

    /// A check the user asked for ("Check for updates" button) or that [`Self::tick`] started.
    /// Returns true when a newer, not-dismissed version is now available.
    pub fn check_now(&self, now_ms: u64) -> bool {
        {
            let mut status = lock(&self.status);
            if status.checking || status.installing {
                return false;
            }
            status.checking = true;
            status.check_error = None;
        }
        self.emit();
        let result = self.backend.check();
        let mut status = lock(&self.status);
        status.checking = false;
        status.can_install = self.backend.can_install();
        status.last_check_ms = Some(now_ms);
        let notify = match result {
            Ok(Some(update)) => {
                let fresh = status.dismissed_version.as_deref() != Some(update.version.as_str());
                status.available = Some(update);
                fresh
            }
            Ok(None) => {
                status.available = None;
                false
            }
            // Offline or server trouble: keep whatever we knew and try again tomorrow.
            Err(error) => {
                status.check_error = Some(error);
                false
            }
        };
        drop(status);
        self.emit();
        notify
    }

    /// The user pressed "Later": hide this version's notification until a newer one appears.
    pub fn dismiss(&self) -> UpdateStatus {
        let mut status = lock(&self.status);
        status.dismissed_version = status.available.as_ref().map(|u| u.version.clone());
        drop(status);
        self.emit();
        self.status()
    }

    /// The user pressed "Install now". Stops background work first; if installing fails the
    /// work is restarted and the error is shown.
    pub fn install(&self) -> Result<(), String> {
        let version = {
            let mut status = lock(&self.status);
            if !status.can_install {
                return Err("This copy of Prism updates by downloading a new version.".into());
            }
            if status.installing || status.checking {
                return Err("An update is already being installed.".into());
            }
            let Some(update) = status.available.clone() else {
                return Err("No update is available.".into());
            };
            status.installing = true;
            status.install_error = None;
            update.version
        };
        self.emit();
        let prepared = std::sync::atomic::AtomicBool::new(false);
        let prepare = || {
            prepared.store(true, std::sync::atomic::Ordering::Release);
            (self.hooks.before_install)()
        };
        let result = self.backend.install(&version, &prepare);
        let mut status = lock(&self.status);
        if let Err(error) = &result {
            status.install_error = Some(error.clone());
        }
        drop(status);
        if result.is_err() && prepared.load(std::sync::atomic::Ordering::Acquire) {
            (self.hooks.after_failed_install)();
        }
        lock(&self.status).installing = false;
        self.emit();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Scripted backend: answers checks from a queue and records installs.
    #[derive(Default)]
    struct Fake {
        answers: Mutex<Vec<Result<Option<AvailableUpdate>, String>>>,
        checks: AtomicUsize,
        installs: Mutex<Vec<String>>,
        install_fails: bool,
        signature_fails: bool,
        executions: AtomicUsize,
        portable: bool,
    }
    impl UpdateBackend for Fake {
        fn check(&self) -> Result<Option<AvailableUpdate>, String> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            self.answers.lock().unwrap().remove(0)
        }
        fn can_install(&self) -> bool {
            !self.portable
        }
        fn install(
            &self,
            version: &str,
            prepare: &dyn Fn() -> Result<(), String>,
        ) -> Result<(), String> {
            self.installs.lock().unwrap().push(version.into());
            if self.signature_fails {
                return Err("signature check failed".into());
            }
            prepare()?;
            self.executions.fetch_add(1, Ordering::SeqCst);
            if self.install_fails {
                Err("installer launch failed".into())
            } else {
                Ok(())
            }
        }
    }
    fn update(version: &str) -> Option<AvailableUpdate> {
        Some(AvailableUpdate {
            version: version.into(),
            notes: String::new(),
        })
    }
    /// A service plus counters for the stop/restart hooks.
    fn make_service(
        fake: Fake,
        auto: bool,
    ) -> (UpdateService, Arc<Fake>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let fake = Arc::new(fake);
        let stopped = Arc::new(AtomicUsize::new(0));
        let restarted = Arc::new(AtomicUsize::new(0));
        let (s, r) = (stopped.clone(), restarted.clone());
        let hooks = InstallHooks {
            before_install: Box::new(move || {
                s.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            after_failed_install: Box::new(move || {
                r.fetch_add(1, Ordering::SeqCst);
            }),
        };
        (
            UpdateService::new(fake.clone(), hooks, auto, None, None),
            fake,
            stopped,
            restarted,
        )
    }
    const DAY: u64 = 24 * 60 * 60 * 1000;
    const LATE: Duration = Duration::from_secs(31);

    #[test]
    fn checks_wait_for_startup_then_run_daily_and_respect_the_setting() {
        let fake = Fake::default();
        *fake.answers.lock().unwrap() = vec![Ok(None), Ok(update("0.2.0"))];
        let (service, fake, ..) = make_service(fake, true);
        assert!(
            !service.tick(Duration::from_secs(5), 1_000),
            "too early after start-up"
        );
        assert_eq!(
            service.next_wait(Duration::from_secs(5), 1_000),
            Duration::from_secs(25)
        );
        assert!(!service.tick(LATE, 1_000), "first check: up to date");
        assert!(!service.tick(LATE, 1_000 + DAY - 1), "not a day yet");
        assert!(service.tick(LATE, 1_000 + DAY), "next day: new version");
        assert_eq!(fake.checks.load(Ordering::SeqCst), 2);
        assert_eq!(service.status().available, update("0.2.0"));
        service.set_auto_check(false);
        assert!(!service.tick(LATE, 1_000 + 3 * DAY));
        assert_eq!(
            fake.checks.load(Ordering::SeqCst),
            2,
            "no checks when turned off"
        );
    }

    #[test]
    fn later_hides_only_the_dismissed_version() {
        let fake = Fake::default();
        *fake.answers.lock().unwrap() = vec![
            Ok(update("0.2.0")),
            Ok(update("0.2.0")),
            Ok(update("0.3.0")),
        ];
        let (service, ..) = make_service(fake, true);
        assert!(service.check_now(1));
        assert_eq!(
            service.dismiss().dismissed_version.as_deref(),
            Some("0.2.0")
        );
        assert!(!service.check_now(2), "same version stays quiet");
        assert!(service.check_now(3), "a newer version is announced again");
    }

    #[test]
    fn offline_checks_are_silent_and_keep_the_known_update() {
        let fake = Fake::default();
        *fake.answers.lock().unwrap() = vec![Ok(update("0.2.0")), Err("offline".into())];
        let (service, ..) = make_service(fake, true);
        assert!(service.check_now(1));
        assert!(!service.check_now(2));
        let status = service.status();
        assert_eq!(status.available, update("0.2.0"));
        assert_eq!(status.install_error, None);
        assert_eq!(
            status.last_check_ms,
            Some(2),
            "retry tomorrow, not every minute"
        );
    }

    #[test]
    fn install_stops_background_work_and_restarts_it_on_failure() {
        let fake = Fake {
            install_fails: true,
            ..Default::default()
        };
        *fake.answers.lock().unwrap() = vec![Ok(update("0.2.0"))];
        let (service, fake, stopped, restarted) = make_service(fake, true);
        assert_eq!(service.install().unwrap_err(), "No update is available.");
        assert_eq!(stopped.load(Ordering::SeqCst), 0);
        service.check_now(1);
        assert_eq!(service.install().unwrap_err(), "installer launch failed");
        assert_eq!(*fake.installs.lock().unwrap(), ["0.2.0"]);
        assert_eq!(
            (
                stopped.load(Ordering::SeqCst),
                restarted.load(Ordering::SeqCst)
            ),
            (1, 1)
        );
        let status = service.status();
        assert!(!status.installing);
        assert_eq!(
            status.install_error.as_deref(),
            Some("installer launch failed")
        );
    }

    #[test]
    fn successful_install_does_not_restart_work_and_portable_never_installs() {
        let fake = Fake::default();
        *fake.answers.lock().unwrap() = vec![Ok(update("0.2.0"))];
        let (service, _, stopped, restarted) = make_service(fake, true);
        service.check_now(1);
        service.install().unwrap();
        assert_eq!(
            (
                stopped.load(Ordering::SeqCst),
                restarted.load(Ordering::SeqCst)
            ),
            (1, 0)
        );

        let portable = Fake {
            portable: true,
            ..Default::default()
        };
        *portable.answers.lock().unwrap() = vec![Ok(update("0.2.0"))];
        let (service, fake, stopped, _) = make_service(portable, true);
        assert!(!service.status().can_install);
        assert!(
            service.check_now(1),
            "portable builds are still told about new versions"
        );
        assert!(service.install().is_err());
        assert!(fake.installs.lock().unwrap().is_empty());
        assert_eq!(stopped.load(Ordering::SeqCst), 0);
        let json = serde_json::to_value(service.status()).unwrap();
        assert_eq!(json["canInstall"], false);
        assert_eq!(json["available"]["version"], "0.2.0");
    }

    #[test]
    fn verification_failure_never_stops_workers_and_prepare_failure_recovers() {
        let fake = Fake {
            signature_fails: true,
            ..Default::default()
        };
        *fake.answers.lock().unwrap() = vec![Ok(update("0.2.0"))];
        let (service, backend, stopped, resumed) = make_service(fake, true);
        service.check_now(1);
        assert!(service.install().is_err());
        assert_eq!(
            (
                stopped.load(Ordering::SeqCst),
                resumed.load(Ordering::SeqCst),
                backend.executions.load(Ordering::SeqCst)
            ),
            (0, 0, 0)
        );
        let backend = Arc::new(Fake::default());
        *backend.answers.lock().unwrap() = vec![Ok(update("0.2.0"))];
        let resumed = Arc::new(AtomicUsize::new(0));
        let counter = resumed.clone();
        let service = UpdateService::new(
            backend.clone(),
            InstallHooks {
                before_install: Box::new(|| Err("cannot save settings".into())),
                after_failed_install: Box::new(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
            },
            true,
            None,
            None,
        );
        service.check_now(1);
        assert_eq!(service.install().unwrap_err(), "cannot save settings");
        assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
        assert_eq!(resumed.load(Ordering::SeqCst), 1);
        assert!(!service.status().installing);
    }

    #[test]
    fn pending_check_preserves_dismissal_and_cannot_race_install() {
        use std::sync::Barrier;
        struct Blocking {
            calls: AtomicUsize,
            entered: Barrier,
            release: Barrier,
        }
        impl UpdateBackend for Blocking {
            fn check(&self) -> Result<Option<AvailableUpdate>, String> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    self.entered.wait();
                    self.release.wait();
                }
                Ok(update("0.2.0"))
            }
            fn can_install(&self) -> bool {
                true
            }
            fn install(&self, _: &str, _: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
                panic!("never install while checking")
            }
        }
        let backend = Arc::new(Blocking {
            calls: AtomicUsize::new(0),
            entered: Barrier::new(2),
            release: Barrier::new(2),
        });
        let service = UpdateService::new(
            backend.clone(),
            InstallHooks {
                before_install: Box::new(|| Ok(())),
                after_failed_install: Box::new(|| {}),
            },
            true,
            None,
            None,
        );
        assert!(service.check_now(1));
        let worker_service = service.clone();
        let worker = std::thread::spawn(move || worker_service.check_now(2));
        backend.entered.wait();
        assert!(service.install().is_err());
        service.dismiss();
        backend.release.wait();
        assert!(!worker.join().unwrap());
        assert_eq!(service.status().dismissed_version.as_deref(), Some("0.2.0"));
        assert!(service.next_wait(Duration::from_secs(30), 2) <= Duration::from_secs(60));
    }
}
