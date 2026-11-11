//! The screen's PIN: needed to open the settings, and optionally to use
//! the dashboard at all.
//!
//! Only a salted hash of the PIN is kept (NVS namespace "security", see
//! `logic::pin`). Once the PIN is entered the screen stays unlocked until it
//! locks again: at power-up, when the screensaver starts, or after the
//! screensaver delay without touches. Home Assistant's "Clear PIN" button
//! and holding BOOT for 5 seconds remove the PIN.

use std::time::Instant;

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys;

use crate::logic::pin::{self, Attempts};

const NVS_NAMESPACE: &str = "security";
const NVS_SALT: &str = "pin_salt";
const NVS_HASH: &str = "pin_hash";
const NVS_LOCK_SCREENS: &str = "lock_screens";

/// Why a PIN was refused.
pub enum Refused {
    /// Wrong; this many tries left before a wait.
    Wrong(u32),
    /// Too many wrong PINs: wait this many seconds.
    Wait(u64),
}

pub struct Lock {
    nvs: Option<EspNvs<NvsDefault>>,
    pin: Option<([u8; 16], [u8; 32])>,
    lock_screens: bool,
    unlocked: bool,
    attempts: Attempts,
    epoch: Instant,
}

impl Lock {
    pub fn new(partition: EspDefaultNvsPartition) -> Self {
        let nvs = EspNvs::new(partition, NVS_NAMESPACE, true)
            .map_err(|e| log::warn!("security settings unavailable: {e}"))
            .ok();
        let mut salt = [0u8; 16];
        let mut hash = [0u8; 32];
        let pin = nvs.as_ref().and_then(|n| {
            let salt_ok = n
                .get_blob(NVS_SALT, &mut salt)
                .ok()
                .flatten()
                .map(|b| b.len())
                == Some(16);
            let hash_ok = n
                .get_blob(NVS_HASH, &mut hash)
                .ok()
                .flatten()
                .map(|b| b.len())
                == Some(32);
            (salt_ok && hash_ok).then_some((salt, hash))
        });
        let lock_screens = nvs
            .as_ref()
            .and_then(|n| n.get_u8(NVS_LOCK_SCREENS).ok().flatten())
            == Some(1);
        log::info!(
            "PIN {}, screens {}",
            if pin.is_some() { "set" } else { "not set" },
            if lock_screens { "locked" } else { "open" }
        );
        Self {
            nvs,
            pin,
            lock_screens,
            unlocked: false,
            attempts: Attempts::default(),
            epoch: Instant::now(),
        }
    }

    pub fn has_pin(&self) -> bool {
        self.pin.is_some()
    }

    pub fn lock_screens(&self) -> bool {
        self.lock_screens
    }

    /// The settings need the PIN now.
    pub fn settings_locked(&self) -> bool {
        self.pin.is_some() && !self.unlocked
    }

    /// The dashboard needs the PIN now.
    pub fn screens_locked(&self) -> bool {
        self.lock_screens && self.settings_locked()
    }

    /// Locks again (screensaver, idle). Returns whether it was unlocked.
    pub fn lock(&mut self) -> bool {
        let was = self.unlocked && self.pin.is_some();
        self.unlocked = false;
        was
    }

    pub fn check(&mut self, entered: &str) -> Result<(), Refused> {
        let now = self.now();
        if let Some(wait) = self.attempts.blocked(now) {
            return Err(Refused::Wait(wait));
        }
        let Some((salt, hash)) = &self.pin else {
            return Ok(());
        };
        if pin::matches(salt, hash, entered) {
            self.attempts.succeeded();
            self.unlocked = true;
            return Ok(());
        }
        log::warn!("wrong PIN entered");
        match self.attempts.failed(now) {
            Some(wait) => Err(Refused::Wait(wait)),
            None => Err(Refused::Wrong(self.attempts.left())),
        }
    }

    /// Sets a new PIN (already checked with `pin::valid`) and stays unlocked.
    pub fn set_pin(&mut self, new: &str) {
        let mut salt = [0u8; 16];
        unsafe { sys::esp_fill_random(salt.as_mut_ptr().cast(), salt.len()) };
        let hash = pin::hash(&salt, new);
        if let Some(nvs) = &mut self.nvs {
            let saved = nvs
                .set_blob(NVS_SALT, &salt)
                .and_then(|()| nvs.set_blob(NVS_HASH, &hash));
            if let Err(e) = saved {
                log::warn!("saving the PIN failed: {e}");
            }
        }
        self.pin = Some((salt, hash));
        self.unlocked = true;
        log::info!("PIN set");
    }

    pub fn clear_pin(&mut self) {
        if let Some(nvs) = &mut self.nvs {
            let _ = nvs.remove(NVS_SALT);
            let _ = nvs.remove(NVS_HASH);
        }
        if self.pin.take().is_some() {
            log::warn!("PIN removed");
        }
        self.attempts.succeeded();
    }

    pub fn set_lock_screens(&mut self, on: bool) {
        self.lock_screens = on;
        if let Some(nvs) = &mut self.nvs {
            if let Err(e) = nvs.set_u8(NVS_LOCK_SCREENS, on as u8) {
                log::warn!("saving the screen lock failed: {e}");
            }
        }
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_secs()
    }
}
