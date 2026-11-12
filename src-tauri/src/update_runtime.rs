use crate::{
    app_paths::{DataLocation, InstallKind},
    update_backend::{self, NativeBackend},
    update_service::{InstallHooks, UpdateBackend, UpdateService, UpdateStatus},
    update_state::UpdateState,
    AppState,
};
use serde::Serialize;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateView {
    #[serde(flatten)]
    pub status: UpdateStatus,
    pub current_version: &'static str,
    pub download_page: &'static str,
    pub install_disabled_reason: Option<&'static str>,
    pub state_error: bool,
}
fn view(status: UpdateStatus, state: &UpdateState, reason: Option<&'static str>) -> UpdateView {
    let install_disabled_reason = if status.can_install {
        None
    } else {
        reason.or(status.available.as_ref().map(|_| "platformMissing"))
    };
    UpdateView {
        status,
        current_version: env!("CARGO_PKG_VERSION"),
        download_page: update_backend::DOWNLOAD_PAGE,
        install_disabled_reason,
        state_error: state.failed(),
    }
}
#[derive(Clone)]
pub struct UpdateRuntime(Arc<Inner>);
struct Inner {
    service: UpdateService,
    state: Arc<UpdateState>,
    reason: Option<&'static str>,
    closed: AtomicBool,
    wake: (Mutex<()>, Condvar),
    isolated: bool,
    #[cfg(debug_assertions)]
    fixture: Option<Arc<crate::update_fixture::MemoryBackend>>,
}
#[derive(Clone)]
struct Recovery {
    scopes: Vec<crate::settings::SearchScope>,
    content_active: bool,
    model_active: bool,
    clipboard_paused: bool,
}
impl UpdateRuntime {
    pub fn new(
        app: AppHandle,
        dir: &Path,
        location: &DataLocation,
        auto_check: bool,
        isolated: bool,
    ) -> Result<Self, String> {
        let state = Arc::new(UpdateState::open(dir.join("updates/state.json"))?);
        let record = state.record();
        let reason = if location.kind == InstallKind::Packaged {
            Some("packaged")
        } else if location.kind == InstallKind::Flatpak {
            // Flatpak owns the read-only `/app` copy and updates it itself.
            Some("flatpak")
        } else if location.kind == InstallKind::Portable {
            Some("portable")
        } else if isolated {
            Some("isolated")
        } else if cfg!(debug_assertions) {
            Some("development")
        } else if update_backend::PUBLIC_KEY.trim().is_empty() {
            Some("signingKeyMissing")
        } else {
            None
        };
        #[cfg(debug_assertions)]
        let fixture = (isolated && location.kind != InstallKind::Packaged)
            .then(|| Arc::new(crate::update_fixture::MemoryBackend::default()));
        let backend: Arc<dyn UpdateBackend> = {
            #[cfg(debug_assertions)]
            if let Some(fixture) = &fixture {
                fixture.clone()
            } else {
                Arc::new(NativeBackend::new(app.clone(), location))
            }
            #[cfg(not(debug_assertions))]
            Arc::new(NativeBackend::new(app.clone(), location))
        };
        let recovery = Arc::new(Mutex::new(None::<Recovery>));
        let before_app = app.clone();
        let before_recovery = recovery.clone();
        let after_app = app.clone();
        let hooks = InstallHooks {
            before_install: Box::new(move || {
                let state = before_app
                    .try_state::<AppState>()
                    .ok_or("Application state unavailable.")?;
                if state
                    .config_error
                    .lock()
                    .map_err(|_| "Settings unavailable.")?
                    .is_some()
                {
                    return Err("Repair application settings before updating.".into());
                }
                let settings = state.settings.lock().map_err(|_| "Settings unavailable.")?;
                crate::settings::write_json(&state.data_dir.join("settings.json"), &*settings)?;
                let session = state
                    .session
                    .lock()
                    .map_err(|_| "Search session unavailable.")?;
                crate::settings::write_json(&state.data_dir.join("session.json"), &*session)?;
                let snapshot = Recovery {
                    scopes: settings.scopes.clone(),
                    content_active: state.content.as_ref().ok().is_some_and(|content| {
                        content
                            .status()
                            .is_ok_and(|status| status.phase != "cancelled")
                    }),
                    model_active: state.embedding.as_ref().ok().is_some_and(|model| {
                        matches!(
                            model.status().phase,
                            crate::embedding_runtime::EmbeddingPhase::Running
                                | crate::embedding_runtime::EmbeddingPhase::Starting
                        )
                    }),
                    clipboard_paused: state.clipboard.status()?.paused,
                };
                drop(session);
                drop(settings);
                *before_recovery
                    .lock()
                    .map_err(|_| "Update recovery unavailable.")? = Some(snapshot);
                if let Ok(documents) = &state.documents {
                    documents.cancel_all();
                }
                if let Ok(rag) = &state.rag {
                    rag.cancel_all();
                }
                if let Ok(content) = &state.content {
                    content.cancel_index();
                }
                state.index.configure(vec![]);
                state.clipboard.set_paused(true)?;
                if let Some(quick) = before_app.get_webview_window("quick") {
                    let _ = quick.hide();
                }
                if let Ok(model) = &state.embedding {
                    model
                        .stop()
                        .map_err(|_| "Could not stop the local search model.")?;
                }
                Ok(())
            }),
            after_failed_install: Box::new(move || {
                let Some(state) = after_app.try_state::<AppState>() else {
                    return;
                };
                let snapshot = recovery.lock().ok().and_then(|mut slot| slot.take());
                if let Some(snapshot) = snapshot {
                    state.index.configure(snapshot.scopes.clone());
                    if snapshot.content_active {
                        if let Ok(content) = &state.content {
                            let _ = content.configure(snapshot.scopes);
                        }
                    }
                    let _ = state.clipboard.set_paused(snapshot.clipboard_paused);
                    if snapshot.model_active {
                        if let Ok(model) = &state.embedding {
                            let _ = model.start();
                        }
                    }
                }
            }),
        };
        let emitter_state = state.clone();
        let service = UpdateService::new(
            backend,
            hooks,
            auto_check,
            record.last_check_ms,
            record.dismissed_version,
        )
        .with_listener(move |status| {
            emitter_state.persist(&status);
            let _ = app.emit("prism:updates", view(status, &emitter_state, reason));
        });
        Ok(Self(Arc::new(Inner {
            service,
            state,
            reason,
            closed: AtomicBool::new(false),
            wake: (Mutex::new(()), Condvar::new()),
            isolated,
            #[cfg(debug_assertions)]
            fixture,
        })))
    }
    pub fn status(&self) -> UpdateView {
        view(self.0.service.status(), &self.0.state, self.0.reason)
    }
    pub fn set_auto_check(&self, enabled: bool) {
        self.0.service.set_auto_check(enabled);
        self.0.wake.1.notify_all();
    }
    pub fn check(&self) -> UpdateView {
        self.0.service.check_now(now_ms());
        self.status()
    }
    pub fn dismiss(&self) -> UpdateView {
        self.0.service.dismiss();
        self.status()
    }
    pub fn install(&self, version: &str) -> Result<(), String> {
        self.0.service.install(version)
    }
    pub fn is_installing(&self) -> bool {
        self.0.service.status().installing
    }
    pub fn shutdown(&self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.wake.1.notify_all();
    }
    pub fn start(&self) {
        if self.0.isolated {
            return;
        }
        let weak = Arc::downgrade(&self.0);
        std::thread::spawn(move || {
            let started = Instant::now();
            loop {
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if inner.closed.load(Ordering::Acquire) {
                    break;
                }
                inner.service.tick(started.elapsed(), now_ms());
                let wait = inner
                    .service
                    .next_wait(started.elapsed(), now_ms())
                    .min(Duration::from_secs(60));
                let guard = inner.wake.0.lock().unwrap_or_else(|e| e.into_inner());
                let _ = inner.wake.1.wait_timeout(guard, wait);
            }
        });
    }
    #[cfg(debug_assertions)]
    pub fn fixture(&self, mode: &str) -> Result<serde_json::Value, String> {
        let backend = self
            .0
            .fixture
            .as_ref()
            .ok_or("Update fixtures require an isolated debug profile.")?;
        if mode != "snapshot" {
            backend.configure(mode)?;
        }
        Ok(backend.snapshot())
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
