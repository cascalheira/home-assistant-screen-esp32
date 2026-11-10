//! Firmware updates (over Wi-Fi from Home Assistant, or from the SD card).
//!
//! Both write an ESP-IDF app image into the spare OTA slot, check its size,
//! SHA-256 (when known) and ESP-IDF's own image validation, then make it the
//! boot slot and restart. The new firmware boots "pending verify": it marks
//! itself valid after running for [`HEALTHY_AFTER`]; if it crashes or resets
//! before that, the bootloader goes back to the previous firmware.

use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use esp_idf_svc::ota::EspOta;
use sha2::{Digest, Sha256};

pub use crate::logic::image::descriptor;

/// How long new firmware must run before it is kept.
pub const HEALTHY_AFTER: Duration = Duration::from_secs(60);

pub const PROJECT: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Firmware file looked for on the SD card at boot.
pub const SD_UPDATE_FILE: &str = "/sdcard/update.bin";

/// Progress shown on screen (and reported to Home Assistant by `ha`).
#[derive(Clone, Debug)]
pub enum Status {
    Started {
        source: &'static str,
        version: String,
    },
    /// 0-100
    Writing(u8),
    Verifying,
    Restarting,
    Failed(String),
}

/// Streams an app image into the spare slot and makes it the boot slot.
///
/// `read` fills the buffer and returns the number of bytes read (0 at the
/// end). `size` is the expected total; `sha256` the expected digest, if
/// known. Does not restart.
pub fn install(
    size: usize,
    sha256: Option<[u8; 32]>,
    mut read: impl FnMut(&mut [u8]) -> Result<usize>,
    mut progress: impl FnMut(Status),
) -> Result<()> {
    let mut ota = EspOta::new().context("OTA unavailable")?;
    let mut update = ota.initiate_update().context("can't start the update")?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 4096];
    let mut written = 0usize;
    let mut last_pct = u8::MAX;
    let mut header_checked = false;

    let result: Result<()> = (|| {
        loop {
            // Fill the whole buffer (network reads can be short) so the first
            // chunk always contains the image header.
            let mut n = 0;
            while n < buf.len() {
                let got = read(&mut buf[n..])?;
                if got == 0 {
                    break;
                }
                n += got;
            }
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            if !header_checked && written == 0 {
                match descriptor(chunk) {
                    Some((project, _)) if project == PROJECT => header_checked = true,
                    Some((project, _)) => bail!("this firmware is for \u{201c}{project}\u{201d}"),
                    None => bail!("not a firmware image"),
                }
            }
            update.write(chunk).context("writing flash")?;
            hasher.update(chunk);
            written += n;
            if written > size {
                bail!("more data than expected");
            }
            let pct = (written * 100 / size.max(1)) as u8;
            if pct != last_pct {
                last_pct = pct;
                progress(Status::Writing(pct));
            }
        }
        progress(Status::Verifying);
        if written != size {
            bail!("incomplete download ({written} of {size} bytes)");
        }
        if let Some(expected) = sha256 {
            if hasher.finalize().as_slice() != expected {
                bail!("checksum mismatch");
            }
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            // Validates the image and switches the boot slot.
            update
                .complete()
                .context("the new firmware failed validation")?;
            log::warn!("firmware update installed ({written} bytes)");
            Ok(())
        }
        Err(e) => {
            let _ = update.abort();
            Err(e)
        }
    }
}

/// Version of firmware that failed to start and was rolled back, if the
/// bootloader recorded one (stays until another update reuses its slot).
pub fn rolled_back_version() -> Option<String> {
    unsafe {
        let part = esp_idf_svc::sys::esp_ota_get_last_invalid_partition();
        if part.is_null() {
            return None;
        }
        let mut desc: esp_idf_svc::sys::esp_app_desc_t = core::mem::zeroed();
        if esp_idf_svc::sys::esp_ota_get_partition_description(part, &mut desc) != 0 {
            return None;
        }
        let raw: Vec<u8> = desc
            .version
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c.to_ne_bytes()[0])
            .collect();
        let version = String::from_utf8_lossy(&raw).into_owned();
        (!version.is_empty() && version != VERSION).then_some(version)
    }
}

/// Keeps the running firmware: cancels a pending rollback. Called once the
/// firmware has run for [`HEALTHY_AFTER`].
pub fn mark_healthy() {
    match EspOta::new().and_then(|mut ota| ota.mark_running_slot_valid()) {
        Ok(()) => log::info!("firmware {VERSION} marked healthy"),
        Err(e) => log::warn!("couldn't mark the firmware healthy: {e}"),
    }
}

/// If the SD card holds a firmware image different from the running one,
/// installs it in a background thread and restarts. The file is renamed
/// afterwards (".installed" or ".failed") so it is applied only once.
pub fn update_from_sd(status: Sender<Status>) {
    let Ok(mut file) = std::fs::File::open(SD_UPDATE_FILE) else {
        return;
    };
    use std::io::Read;
    let mut header = [0u8; 112];
    if file.read_exact(&mut header).is_err() {
        return;
    }
    let size = file.metadata().map(|m| m.len() as usize).unwrap_or(0);
    let Some((project, version)) = descriptor(&header) else {
        log::warn!("{SD_UPDATE_FILE} is not a firmware image; ignoring it");
        let _ = std::fs::rename(SD_UPDATE_FILE, format!("{SD_UPDATE_FILE}.failed"));
        return;
    };
    if project != PROJECT {
        log::warn!("{SD_UPDATE_FILE} is for {project}; ignoring it");
        let _ = std::fs::rename(SD_UPDATE_FILE, format!("{SD_UPDATE_FILE}.failed"));
        return;
    }
    if version == VERSION {
        log::info!("{SD_UPDATE_FILE} is the running version {VERSION}; nothing to do");
        return;
    }
    drop(file);
    log::warn!("updating firmware {VERSION} -> {version} from the SD card");

    let spawned = std::thread::Builder::new()
        .name("sd-update".into())
        .stack_size(16 * 1024)
        .spawn(move || {
            let _ = status.send(Status::Started {
                source: "SD card",
                version: version.clone(),
            });
            let result = std::fs::File::open(SD_UPDATE_FILE)
                .context("can't open the file")
                .and_then(|mut file| {
                    install(
                        size,
                        None,
                        |buf| Ok(file.read(buf)?),
                        |s| {
                            let _ = status.send(s);
                        },
                    )
                });
            match result {
                Ok(()) => {
                    let _ = std::fs::rename(SD_UPDATE_FILE, format!("{SD_UPDATE_FILE}.installed"));
                    let _ = status.send(Status::Restarting);
                    std::thread::sleep(Duration::from_secs(2));
                    unsafe { esp_idf_svc::sys::esp_restart() };
                }
                Err(e) => {
                    log::error!("SD card update failed: {e:#}");
                    let _ = std::fs::rename(SD_UPDATE_FILE, format!("{SD_UPDATE_FILE}.failed"));
                    let _ = status.send(Status::Failed(format!("{e:#}")));
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("can't start the SD card update: {e}");
    }
}

/// Debug aid for testing SD-card updates: copies the other (not running)
/// firmware slot, whole, to [`SD_UPDATE_FILE`]. The trailing erased bytes
/// are harmless; the image header says where the firmware ends.
pub fn copy_spare_slot_to_sd() -> Result<usize> {
    use std::io::Write;
    unsafe {
        let part = esp_idf_svc::sys::esp_ota_get_next_update_partition(core::ptr::null());
        if part.is_null() {
            bail!("no spare firmware slot");
        }
        let size = (*part).size as usize;
        let mut file = std::fs::File::create(SD_UPDATE_FILE)?;
        let mut buf = vec![0u8; 4096];
        let mut offset = 0;
        while offset < size {
            let n = buf.len().min(size - offset);
            esp_idf_svc::sys::esp!(esp_idf_svc::sys::esp_partition_read(
                part,
                offset,
                buf.as_mut_ptr().cast(),
                n
            ))?;
            file.write_all(&buf[..n])?;
            offset += n;
        }
        Ok(size)
    }
}
