//! Lifecycle supervisor for the active capture backend.
//!
//! One supervisor instance multiplexes the two backends
//! (`HudsuckerBackend`, `ChromiumBackend`) — the one that runs is
//! determined by `cfg.capture.mode`. Owns `state.capture_control`
//! (retained cancellation/completion and force-close `Notify`) and emits onto
//! `state.capture_status_bus`.
//!
//! Start/stop/restart operations are serialised. Each run retains both its
//! cancellation request and completion signal; a timeout never permits a
//! replacement to race a backend that is still cleaning up.

use crate::capture::{
    chromium::ChromiumBackend, hudsucker_backend::HudsuckerBackend, CaptureBackend, CaptureCtx,
    CaptureKind as RtCaptureKind, ShutdownToken,
};
use crate::config::CaptureMode;
use crate::ipc::state::{AppState, CaptureControl};
use crate::schema::{CaptureKind, CaptureStatus, Notification};
use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex};
use tracing::{error, info};

const STOP_TIMEOUT: Duration = Duration::from_secs(20);

fn schema_kind(k: RtCaptureKind) -> CaptureKind {
    match k {
        RtCaptureKind::Mitm => CaptureKind::Mitm,
        RtCaptureKind::Chromium => CaptureKind::Chromium,
    }
}

/// Await this run's actual completion, retaining its control state on timeout
/// or caller cancellation. Caller holds the lifecycle operation lock.
async fn stop_and_wait(control: &Arc<Mutex<CaptureControl>>, max_wait: Duration) -> Result<bool> {
    let (stop, mut done, force_close) = {
        let ctl = control.lock().await;
        if !ctl.is_running() {
            return Ok(false);
        }
        (
            ctl.stop.clone(),
            ctl.done
                .clone()
                .expect("running capture has completion signal"),
            ctl.force_close.clone(),
        )
    };
    force_close.notify_waiters();
    if let Some(stop) = stop {
        stop.send_replace(true);
    }
    tokio::time::timeout(max_wait, done.wait_for(|done| *done))
        .await
        .context("capture shutdown timed out; backend is still stopping")?
        .context("capture task exited without reporting completion")?;
    Ok(true)
}

/// Stop the backend and wait for cleanup. Returns an error if already stopped.
pub async fn stop_capture(state: AppState) -> Result<()> {
    let _operation = state.capture_operation.lock().await;
    if !stop_and_wait(&state.capture_control, STOP_TIMEOUT).await? {
        bail!("capture backend is not running");
    }
    Ok(())
}

/// Stop the running backend (if any) and start a fresh one based on the
/// current `cfg.capture.mode`. Idempotent: safe to call when nothing is
/// running (becomes a plain `spawn`).
pub async fn restart_capture(state: AppState) -> Result<()> {
    let _operation = state.capture_operation.lock().await;
    stop_and_wait(&state.capture_control, STOP_TIMEOUT).await?;
    start_backend(&state).await
}

/// Spawn the capture supervisor task and wire it into
/// `state.capture_control`. Returns once the task has been spawned (not
/// when the backend is actually live).
pub async fn spawn_capture_supervisor(state: AppState) -> Result<()> {
    let _operation = state.capture_operation.lock().await;
    start_backend(&state).await
}

async fn start_backend(state: &AppState) -> Result<()> {
    {
        let ctl = state.capture_control.lock().await;
        if ctl.is_running() {
            bail!("capture backend already running or stopping");
        }
    }

    let (mode, proxy_cfg, chromium_cfg, http_cfg, platform) = {
        let cfg = state.config.read().await;
        (
            cfg.capture.mode,
            cfg.proxy.clone(),
            cfg.capture.chromium.clone(),
            cfg.capture.http.clone(),
            cfg.platform.kind,
        )
    };

    let (shutdown_token, shutdown_cancel) = ShutdownToken::new();
    let (done_tx, done_rx) = watch::channel(false);

    // Build backend per mode.
    let backend: Box<dyn CaptureBackend> = match mode {
        CaptureMode::Mitm => {
            let force_close = {
                let ctl = state.capture_control.lock().await;
                ctl.force_close.clone()
            };
            Box::new(HudsuckerBackend::new(
                proxy_cfg.clone(),
                http_cfg.clone(),
                force_close,
            ))
        }
        CaptureMode::Chromium => Box::new(ChromiumBackend::new(chromium_cfg)),
    };
    let descriptor = backend.descriptor();
    let kind = schema_kind(descriptor.kind);

    // Status: emit Running with a descriptive label up front so the UI
    // sees an immediate transition from Stopped → Running. Backends that
    // fail mid-startup will flip back to Error from the spawned task.
    let label = match descriptor.kind {
        RtCaptureKind::Mitm => proxy_cfg.addr.clone(),
        RtCaptureKind::Chromium => format!("chromium ({})", descriptor.label),
    };
    let running_status = CaptureStatus::Running {
        kind,
        descriptor: label.clone(),
    };
    {
        let mut ctl = state.capture_control.lock().await;
        ctl.status = running_status.clone();
        ctl.stop = Some(shutdown_cancel);
        ctl.done = Some(done_rx);
    }
    let _ = state.capture_status_bus.send(running_status);

    let ctx = CaptureCtx {
        session: state.log_session.clone(),
        platform,
        mjai_bus: state.mjai_bus.clone(),
        notify_bus: state.notify_bus.clone(),
        autoplay: Some(state.autoplay_context.clone()),
        http: http_cfg,
    };
    let status_bus = state.capture_status_bus.clone();
    let control = state.capture_control.clone();
    let notify_bus = state.notify_bus.clone();

    info!(
        "capture supervisor: starting {} backend ({})",
        descriptor.kind.as_str(),
        label
    );
    tokio::spawn(async move {
        let result = backend.run(ctx, shutdown_token).await;
        let next = match &result {
            Ok(()) => {
                info!("capture supervisor: stopped cleanly");
                CaptureStatus::Stopped
            }
            Err(e) => {
                let msg = format!("{e:#}");
                error!("capture supervisor: {msg}");
                // Surface via toast so the user sees it without watching
                // the dashboard. Sticky id `capture-error` lets the next
                // restart's Running emission overwrite the same toast.
                let _ = notify_bus.send(
                    Notification::error(format!("{} capture stopped", kind_label(kind)))
                        .body(format!(
                            "{msg} — open Settings → Capture and click Restart."
                        ))
                        .sticky()
                        .id("capture-error"),
                );
                CaptureStatus::Error {
                    kind,
                    descriptor: Some(label.clone()),
                    error: msg,
                }
            }
        };
        {
            let mut ctl = control.lock().await;
            ctl.stop = None;
            ctl.status = next.clone();
        }
        let _ = status_bus.send(next);
        done_tx.send_replace(true);
    });

    Ok(())
}

fn kind_label(k: CaptureKind) -> &'static str {
    match k {
        CaptureKind::Mitm => "MITM",
        CaptureKind::Chromium => "Chromium",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timed_out_stop_retains_the_stopping_run() {
        let (stop, cancel) = watch::channel(false);
        let (finish, done) = watch::channel(false);
        let control = Arc::new(Mutex::new(CaptureControl {
            stop: Some(stop),
            done: Some(done),
            ..Default::default()
        }));
        let error = stop_and_wait(&control, Duration::from_millis(1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(*cancel.borrow());
        assert!(control.lock().await.is_running());
        assert!(control.lock().await.stop.is_some());
        finish.send_replace(true);
        assert!(!control.lock().await.is_running());
        assert!(!stop_and_wait(&control, Duration::from_millis(100))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn cancelled_stop_call_retains_completion_and_can_be_retried() {
        let (stop, mut cancel) = watch::channel(false);
        let (finish, done) = watch::channel(false);
        let control = Arc::new(Mutex::new(CaptureControl {
            stop: Some(stop),
            done: Some(done),
            ..Default::default()
        }));
        let mut stopping = Box::pin(stop_and_wait(&control, Duration::from_secs(1)));
        tokio::select! {
            result = &mut stopping => panic!("stop returned before cleanup: {result:?}"),
            result = cancel.wait_for(|cancelled| *cancelled) => { assert!(result.is_ok()); }
        }
        drop(stopping);
        assert!(control.lock().await.is_running());
        let retry = stop_and_wait(&control, Duration::from_secs(1));
        let complete = async {
            tokio::task::yield_now().await;
            finish.send_replace(true);
        };
        let (result, ()) = tokio::join!(retry, complete);
        assert!(result.unwrap());
        assert!(!control.lock().await.is_running());
    }
}
