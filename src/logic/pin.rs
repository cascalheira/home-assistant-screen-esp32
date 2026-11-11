//! The screen's PIN: format, how it is stored, and slowing down guessing.

use sha2::{Digest, Sha256};

pub const MIN_LEN: usize = 4;
pub const MAX_LEN: usize = 8;
/// Wrong PINs in a row before the pad waits.
pub const MAX_FAILURES: u32 = 5;
pub const BLOCK_SECS: u64 = 30;
/// SHA-256 rounds: a few milliseconds on the ESP32-S3, enough that a copy
/// of the stored hash doesn't give up a 4-digit PIN instantly.
const ROUNDS: u32 = 2000;

/// 4 to 8 digits.
pub fn valid(pin: &str) -> bool {
    (MIN_LEN..=MAX_LEN).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

/// What is stored instead of the PIN.
pub fn hash(salt: &[u8; 16], pin: &str) -> [u8; 32] {
    let mut digest: [u8; 32] = Sha256::new()
        .chain_update(salt)
        .chain_update(pin.as_bytes())
        .finalize()
        .into();
    for _ in 1..ROUNDS {
        digest = Sha256::new()
            .chain_update(digest)
            .chain_update(salt)
            .finalize()
            .into();
    }
    digest
}

/// Compares without stopping at the first different byte.
pub fn matches(salt: &[u8; 16], stored: &[u8; 32], pin: &str) -> bool {
    hash(salt, pin)
        .iter()
        .zip(stored)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// Wrong attempts in a row; times are seconds since any fixed point.
#[derive(Default, Debug)]
pub struct Attempts {
    failures: u32,
    blocked_until: Option<u64>,
}

impl Attempts {
    /// Seconds left before another try is allowed, if blocked.
    pub fn blocked(&self, now: u64) -> Option<u64> {
        self.blocked_until
            .filter(|&until| until > now)
            .map(|until| until - now)
    }

    /// Records a wrong PIN. Returns the seconds to wait, if it now blocks.
    pub fn failed(&mut self, now: u64) -> Option<u64> {
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            self.failures = 0;
            self.blocked_until = Some(now + BLOCK_SECS);
            return Some(BLOCK_SECS);
        }
        None
    }

    pub fn succeeded(&mut self) {
        *self = Self::default();
    }

    /// Tries left before a wait.
    pub fn left(&self) -> u32 {
        MAX_FAILURES - self.failures
    }
}
