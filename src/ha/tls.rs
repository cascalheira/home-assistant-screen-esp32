//! HTTPS trust for every connection to Home Assistant.
//!
//! By default certificates are checked against ESP-IDF's bundle of public
//! certificate authorities. A Home Assistant with a self-signed certificate
//! fails that check; the user can then trust its certificate on the screen
//! (trust on first use). From then on that server is accepted only with
//! exactly that certificate, compared by its SHA-256 fingerprint. Proving
//! possession of the certificate's private key is part of the TLS
//! handshake, so a pinned certificate can't be replayed by someone else.
//!
//! The trust decision is installed through `crt_bundle_attach`, the hook
//! ESP-IDF's HTTP and websocket clients call while setting up mbedTLS.

use core::ffi::{c_int, c_void};
use core::mem::MaybeUninit;
use std::cell::UnsafeCell;
use std::sync::{Mutex, Once};

use esp_idf_svc::sys;
use sha2::{Digest, Sha256};

pub type Fingerprint = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Public certificate authorities (ESP-IDF's bundle).
    Bundle,
    /// Only this server certificate.
    Pinned(Fingerprint),
    /// Record the server's certificate and refuse the connection.
    Probe,
}

static MODE: Mutex<Mode> = Mutex::new(Mode::Bundle);
/// The server certificate seen by a probe, or refused by the pin.
static SEEN: Mutex<Option<Fingerprint>> = Mutex::new(None);

/// Pass as `crt_bundle_attach` in every client configuration.
///
/// # Safety
/// Called by ESP-IDF with a valid `mbedtls_ssl_config`.
pub unsafe extern "C" fn attach(conf: *mut c_void) -> sys::esp_err_t {
    let mode = *MODE.lock().unwrap();
    match mode {
        Mode::Bundle => unsafe { sys::esp_crt_bundle_attach(conf) },
        Mode::Pinned(_) | Mode::Probe => {
            let conf = conf.cast::<sys::mbedtls_ssl_config>();
            unsafe {
                // mbedTLS refuses to verify without a CA chain; like ESP-IDF's
                // bundle, hand it an empty one and decide in the callback.
                sys::mbedtls_ssl_conf_ca_chain(conf, empty_ca(), core::ptr::null_mut());
                sys::mbedtls_ssl_conf_verify(conf, Some(verify), core::ptr::null_mut());
            }
            sys::ESP_OK
        }
    }
}

/// Called by mbedTLS for each certificate in the server's chain, from the
/// top down to the server's own certificate (depth 0). Clearing `flags`
/// accepts that certificate.
unsafe extern "C" fn verify(
    _ctx: *mut c_void,
    crt: *mut sys::mbedtls_x509_crt,
    depth: c_int,
    flags: *mut u32,
) -> c_int {
    if depth != 0 {
        // Only the server's own certificate decides.
        unsafe { *flags = 0 };
        return 0;
    }
    let raw = unsafe { &(*crt).raw };
    let der = unsafe { core::slice::from_raw_parts(raw.p, raw.len) };
    let fingerprint: Fingerprint = Sha256::digest(der).into();
    let mode = *MODE.lock().unwrap();
    match mode {
        Mode::Pinned(pin) if pin == fingerprint => unsafe { *flags = 0 },
        Mode::Pinned(_) | Mode::Probe => {
            // Keep the flags set (or set one): the handshake fails.
            *SEEN.lock().unwrap() = Some(fingerprint);
            unsafe { *flags |= sys::MBEDTLS_X509_BADCERT_NOT_TRUSTED };
        }
        // Not installed in this mode.
        Mode::Bundle => {}
    }
    0
}

/// An empty certificate list, initialised once.
fn empty_ca() -> *mut sys::mbedtls_x509_crt {
    struct Empty(UnsafeCell<MaybeUninit<sys::mbedtls_x509_crt>>);
    // SAFETY: written once under `INIT`, then only read by mbedTLS.
    unsafe impl Sync for Empty {}
    static EMPTY: Empty = Empty(UnsafeCell::new(MaybeUninit::uninit()));
    static INIT: Once = Once::new();
    let crt = EMPTY.0.get().cast::<sys::mbedtls_x509_crt>();
    INIT.call_once(|| unsafe { sys::mbedtls_x509_crt_init(crt) });
    crt
}

/// Trust exactly `pin` from now on, or public authorities with `None`.
pub fn set_pin(pin: Option<Fingerprint>) {
    *MODE.lock().unwrap() = pin.map_or(Mode::Bundle, Mode::Pinned);
}

/// Runs `connect` (any request to the server) in probe mode and returns the
/// fingerprint of the certificate the server presented, if the connection
/// got that far. The previous mode is restored.
pub fn probe(connect: impl FnOnce()) -> Option<Fingerprint> {
    let previous = std::mem::replace(&mut *MODE.lock().unwrap(), Mode::Probe);
    *SEEN.lock().unwrap() = None;
    connect();
    *MODE.lock().unwrap() = previous;
    SEEN.lock().unwrap().take()
}

/// The certificate a pinned connection refused since the last call, if any:
/// the server's certificate changed.
pub fn take_refused() -> Option<Fingerprint> {
    let refused = SEEN.lock().unwrap().take();
    let pinned = matches!(*MODE.lock().unwrap(), Mode::Pinned(_));
    refused.filter(|_| pinned)
}

/// `AB:CD:…`, as `openssl x509 -noout -fingerprint -sha256` prints it.
pub fn format(fingerprint: &Fingerprint) -> String {
    crate::logic::net::fingerprint_text(fingerprint)
}
