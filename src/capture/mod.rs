//! Capture transports — the layer that supplies WebSocket frames to the
//! [`crate::bridge::Bridge`] parser.
//!
//! Two backends today:
//! - [`hudsucker_backend::HudsuckerBackend`] — MITM proxy (legacy, requires
//!   system proxy + CA cert install).
//! - [`chromium::ChromiumBackend`] — controlled Chromium browser, intercepts
//!   WebSocket frames via CDP. No proxy/CA setup.
//!
//! Both implement [`CaptureBackend`] and feed frames through [`flow::FlowBridges`]
//! into the platform bridge, which emits mjai events on [`crate::event_bus::MjaiBus`].

pub mod chromium;
pub mod flow;
pub mod http;
pub mod hudsucker_backend;

use crate::autoplay::AutoplayContext;
use crate::config::Platform;
use crate::event_bus::{MjaiBus, NotifyBus};
use crate::logger::Session;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::watch;

/// Discriminant exposed in [`CaptureStatus`] / IPC payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Mitm,
    Chromium,
}

impl CaptureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureKind::Mitm => "mitm",
            CaptureKind::Chromium => "chromium",
        }
    }
}

/// Static per-backend descriptor. The supervisor surfaces this in
/// status messages so the UI can show "Running (Chromium @ /usr/bin/google-chrome)"
/// or similar.
#[derive(Debug, Clone)]
pub struct CaptureDescriptor {
    pub kind: CaptureKind,
    /// Human-readable description (listen addr for MITM, executable path
    /// for Chromium). Surface in UI; do not parse.
    pub label: String,
}

/// Retained shutdown signal. Cancellation reaches both current and future
/// waiters, including a backend that is still starting.
#[derive(Clone)]
pub struct ShutdownToken(watch::Receiver<bool>);

impl ShutdownToken {
    pub fn new() -> (Self, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(false);
        (Self(rx), tx)
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves on cancellation or when the supervisor drops its sender.
    pub async fn wait(&self) {
        let mut rx = self.0.clone();
        let _ = rx.wait_for(|cancelled| *cancelled).await;
    }
}

/// Everything a backend needs to push parsed mjai events into the rest of
/// the app. Cheap to clone (all `Arc`/`Sender`/`Notify`).
#[derive(Clone)]
pub struct CaptureCtx {
    pub session: Arc<Session>,
    pub platform: Platform,
    pub mjai_bus: MjaiBus,
    pub notify_bus: NotifyBus,
    /// Shared with the autoplay manager. The chromium backend writes the
    /// per-tab `Page` handle here when it observes a Majsoul WS;
    /// autoplay reads it to dispatch `Input.dispatchMouseEvent`. The
    /// MITM backend simply ignores this — it has no `Page`.
    pub autoplay: Option<Arc<AutoplayContext>>,
    /// What to record of the HTTP traffic this backend intercepts. Both
    /// backends honour it; `static_assets` is chromium-only.
    pub http: crate::config::HttpCaptureConfig,
}

/// A capture transport. Implementors run a long-lived I/O loop until
/// `shutdown` resolves or a fatal error occurs, pushing frames into the
/// bridge layer along the way.
#[async_trait]
pub trait CaptureBackend: Send {
    /// Run the backend. Must respect `shutdown` for graceful exit.
    /// Returns `Ok(())` on clean exit, `Err` otherwise.
    async fn run(self: Box<Self>, ctx: CaptureCtx, shutdown: ShutdownToken) -> Result<()>;

    /// Static description for status reporting. Called once at spawn time
    /// (and again after restart) — must not block.
    fn descriptor(&self) -> CaptureDescriptor;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn shutdown_reaches_late_and_multiple_waiters() {
        let (token, cancel) = ShutdownToken::new();
        cancel.send_replace(true);
        assert!(token.is_cancelled());
        let clone = token.clone();
        tokio::time::timeout(Duration::from_millis(100), async {
            tokio::join!(token.wait(), token.wait(), clone.wait());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn shutdown_reaches_existing_waiters_and_sender_drop() {
        let (token, cancel) = ShutdownToken::new();
        let waiter = tokio::spawn(async move { token.wait().await });
        tokio::task::yield_now().await;
        cancel.send_replace(true);
        tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .unwrap()
            .unwrap();
        let (token, cancel) = ShutdownToken::new();
        drop(cancel);
        tokio::time::timeout(Duration::from_millis(100), token.wait())
            .await
            .unwrap();
    }
}
