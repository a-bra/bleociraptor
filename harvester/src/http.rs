// ABOUTME: HTTP transport (§12) — owns sockets and routes, renders nothing:
// ABOUTME: every body comes from harvester_core::render::* into the socket.

use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use harvester_core::render::{self, Health};
use harvester_core::{Clock, Registry};
use std::fmt;

use crate::config;
use crate::effects::{self, EspClock};
use crate::state::{HOT, LOGS, SPARK};

/// Snapshot of everything /metrics and /api/readings need beyond the registry.
/// Assembled per request; cheap.
fn health(
    wifi_rssi: Option<i8>,
    reboots: render::RebootCounts,
    last_reason: &'static str,
) -> Health {
    Health {
        uptime_seconds: EspClock.monotonic().as_secs(),
        free_heap_bytes: effects::free_heap_bytes(),
        min_free_heap_bytes: effects::min_free_heap_bytes(),
        wifi_rssi_dbm: wifi_rssi,
        version: env!("CARGO_PKG_VERSION"),
        git_sha: env!("HARVESTER_GIT_SHA"),
        reboots,
        last_reboot_reason: last_reason,
    }
}

/// Bridge: core renders into fmt::Write; the socket wants bytes. Renders happen
/// from a SNAPSHOT, never under HOT (§6.2 rule 3).
struct FmtBridge<W>(W);

impl<W: esp_idf_svc::io::Write> fmt::Write for FmtBridge<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

/// Shared per-request context the routes need; filled by main at wiring time.
pub struct HttpDeps {
    pub wifi_rssi: fn() -> Option<i8>,
    pub reboots: render::RebootCounts,
    pub last_reboot_reason: &'static str,
}

pub fn serve(deps: &'static HttpDeps) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        http_port: config::LISTEN_PORT,
        // §6.2: a 2.3 KB snapshot + JSON formatting + call frames exhaust the
        // 4096 default; overflow presents as reboot_total{reason="panic"}
        // climbing whenever the dashboard is open. One task, so one stack.
        stack_size: 10240,
        max_open_sockets: 5, // §12
        ..Default::default()
    })?;

    server.fn_handler("/healthz", Method::Get, |req| {
        req.into_ok_response()?
            .write(b"ok")
            .map(|_| ())
            .map_err(anyhow::Error::from)
    })?;

    server.fn_handler("/status", Method::Get, |req| {
        let active = HOT
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(Registry::devices_active))
            .unwrap_or(0);
        let mut resp = req.into_response(200, None, &[("Content-Type", "application/json")])?;
        let mut w = FmtBridge(&mut resp);
        render::status::render_status(&mut w, config::DEVICE_TIMEOUT.as_secs(), active)
            .map_err(|_| anyhow::anyhow!("socket write failed"))
    })?;

    server.fn_handler("/metrics", Method::Get, move |req| {
        // Snapshot under HOT, render outside it (§6.2 rule 3).
        let snapshot = HOT.lock().ok().and_then(|g| g.as_ref().cloned());
        let Some(registry) = snapshot else {
            req.into_status_response(503)?;
            return Ok(());
        };
        let now = EspClock.monotonic();
        let unix = EspClock.unix_seconds();
        let h = health((deps.wifi_rssi)(), deps.reboots, deps.last_reboot_reason);
        let mut resp = req.into_response(
            200,
            None,
            &[("Content-Type", "text/plain; version=0.0.4; charset=utf-8")],
        )?;
        let mut w = FmtBridge(&mut resp);
        render::prometheus::render_metrics(&mut w, &registry, &h, now, unix)
            .map_err(|_| anyhow::anyhow!("socket write failed"))
    })?;

    server.fn_handler("/api/readings", Method::Get, move |req| {
        let snapshot = HOT.lock().ok().and_then(|g| g.as_ref().cloned());
        let Some(registry) = snapshot else {
            req.into_status_response(503)?;
            return Ok(());
        };
        let now = EspClock.monotonic();
        let unix = EspClock.unix_seconds();
        let h = health((deps.wifi_rssi)(), deps.reboots, deps.last_reboot_reason);
        let mut resp = req.into_response(200, None, &[("Content-Type", "application/json")])?;
        let mut w = FmtBridge(&mut resp);
        render::readings::render_readings(&mut w, &registry, &h, now, unix)
            .map_err(|_| anyhow::anyhow!("socket write failed"))
    })?;

    server.fn_handler("/api/history", Method::Get, |req| {
        let registry = HOT.lock().ok().and_then(|g| g.as_ref().cloned());
        let Some(registry) = registry else {
            req.into_status_response(503)?;
            return Ok(());
        };
        // SPARK is copied (~2.3 KB) so the lock is held for a memcpy, not a render.
        let spark = SPARK
            .lock()
            .map(|s| s.clone())
            .map_err(|_| anyhow::anyhow!("poisoned"))?;
        let mut resp = req.into_response(200, None, &[("Content-Type", "application/json")])?;
        let mut w = FmtBridge(&mut resp);
        render::history::render_history(
            &mut w,
            &registry,
            &spark,
            config::SPARK_SAMPLE_INTERVAL.as_secs(),
        )
        .map_err(|_| anyhow::anyhow!("socket write failed"))
    })?;

    server.fn_handler("/logs", Method::Get, |req| {
        let now = EspClock.monotonic();
        let unix = EspClock.unix_seconds();
        let mut resp =
            req.into_response(200, None, &[("Content-Type", "text/plain; charset=utf-8")])?;
        let mut w = FmtBridge(&mut resp);
        // §6.2 rule 4: /logs is the exception — it streams UNDER the LOGS lock.
        // Only rare failure-path writes contend; a fetch can delay a log write,
        // never a beacon.
        let ring = LOGS.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
        ring.render(&mut w, now, unix)
            .map_err(|_| anyhow::anyhow!("socket write failed"))
    })?;

    Ok(server)
}
