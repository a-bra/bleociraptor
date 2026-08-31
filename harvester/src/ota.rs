// ABOUTME: OTA effects (§13) — stream a firmware image into the other slot behind
// ABOUTME: X-Auth; the RAII guard keeps the reboot paths suppressed and always released.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write as _;
use esp_idf_svc::ota::EspOta;

use crate::devices;

/// True while a firmware body is streaming. Suppresses the wedge and Wi-Fi
/// giveup reboots (§13): esp_restart() mid-write corrupts the update.
pub static OTA_IN_PROGRESS: Mutex<bool> = Mutex::new(false);

/// Set by the handler after a completed upload; the housekeeping sweep performs
/// the actual restart through the ledger so the reason lands in NVS (§11.2).
pub static OTA_RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

/// RAII (§13): the flag must clear on EVERY exit path — a dropped connection,
/// a bad body, a write error. One left set would disable both recovery reboots
/// permanently and invisibly, which is worse than the corruption it prevents.
struct UploadGuard;

impl UploadGuard {
    fn acquire() -> Option<Self> {
        let mut flag = OTA_IN_PROGRESS.lock().ok()?;
        if *flag {
            return None; // an upload is already streaming
        }
        *flag = true;
        Some(Self)
    }
}

impl Drop for UploadGuard {
    fn drop(&mut self) {
        if let Ok(mut flag) = OTA_IN_PROGRESS.lock() {
            *flag = false;
        }
    }
}

pub fn register(server: &mut EspHttpServer<'static>) -> anyhow::Result<()> {
    server.fn_handler("/ota", Method::Post, |mut req| {
        // LAN-only surface, but this is the one endpoint that must never be
        // open (§12): constant-shape token check before touching flash.
        let authorized = req
            .header("X-Auth")
            .is_some_and(|h| h.as_bytes() == devices::OTA_TOKEN.as_bytes());
        if !authorized {
            req.into_status_response(401)?;
            return Ok(());
        }
        let Some(_guard) = UploadGuard::acquire() else {
            req.into_status_response(409)?; // concurrent upload
            return Ok(());
        };

        let mut ota = EspOta::new()?;
        let mut update = ota.initiate_update()?;
        let mut buf = [0u8; 2048];
        let mut total = 0usize;
        loop {
            let n = match req.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    update.abort()?;
                    log::warn!("ota read failed after {total} bytes: {e:?}");
                    req.into_status_response(500)?;
                    return Ok(());
                }
            };
            if let Err(e) = update.write(buf.get(..n).unwrap_or(&[])) {
                update.abort()?;
                log::warn!("ota write failed after {total} bytes: {e:?}");
                req.into_status_response(500)?;
                return Ok(());
            }
            total += n;
        }
        if total == 0 {
            update.abort()?;
            req.into_status_response(400)?;
            return Ok(());
        }
        update.complete()?;
        log::info!("ota complete: {total} bytes; restart queued");
        // The housekeeping sweep restarts via the ledger within a second, so
        // reboot_total{reason=\"ota\"} is accounted (§11.2) and this response
        // still reaches the client.
        OTA_RESTART_REQUESTED.store(true, Ordering::SeqCst);
        req.into_ok_response()?
            .write_all(format!("ok: {total} bytes, rebooting\n").as_bytes())?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

/// §13 validation, called from the 1 Hz sweep. Marks the running slot valid
/// after OTA_VALIDATE_AFTER of healthy uptime AND any radio evidence (a parsed
/// beacon OR a stray advert — the OR is load-bearing: requiring a parsed beacon
/// alone would boot-loop a good build on a bench with no sensors in range), OR
/// unconditionally at OTA_VALIDATE_DEADLINE. Requires
/// CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y or it is a silent no-op.
pub fn validate_if_due(uptime_secs: u64, radio_evidence: bool, already_valid: &mut bool) {
    if *already_valid {
        return;
    }
    let after = crate::config::OTA_VALIDATE_AFTER.as_secs();
    let deadline = crate::config::OTA_VALIDATE_DEADLINE.as_secs();
    let due = (uptime_secs >= after && radio_evidence) || uptime_secs >= deadline;
    if !due {
        return;
    }
    match EspOta::new().and_then(|mut o| o.mark_running_slot_valid()) {
        Ok(()) => {
            *already_valid = true;
            log::info!("running slot marked valid at uptime {uptime_secs}s");
        }
        Err(e) => {
            // Already-valid slots report an error here; treat as settled.
            *already_valid = true;
            log::info!("mark_running_slot_valid: {e:?} (slot presumably already valid)");
        }
    }
}
