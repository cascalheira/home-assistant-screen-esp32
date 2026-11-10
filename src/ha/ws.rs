//! Thin wrapper over ESP-IDF's `esp_websocket_client`.
//!
//! The esp-idf-svc wrapper hands out each received chunk separately, and a
//! layout can be larger than one chunk. This wrapper reassembles complete
//! text messages and forwards them, plus connect/disconnect notifications,
//! to a channel.

use core::ffi::{c_char, c_void};
use std::ffi::CString;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{bail, Result};
use esp_idf_svc::sys;

pub enum WsEvent {
    Connected,
    Disconnected,
    Text(String),
}

struct Context {
    events: Sender<super::Msg>,
    generation: u32,
    buffer: Vec<u8>,
}

pub struct WebSocket {
    handle: sys::esp_websocket_client_handle_t,
    // Referenced by the C client; freed only after the client is destroyed.
    context: *mut Context,
    _uri: CString,
}

// SAFETY: the client handle is internally locked by ESP-IDF; `context` is
// only touched by the client's own task until `Drop`, which destroys the
// client (joining that task) before freeing it.
unsafe impl Send for WebSocket {}

impl WebSocket {
    /// Connects in the background. Events arrive tagged with `generation` so
    /// stale events from a previous connection can be told apart.
    pub fn connect(uri: &str, events: Sender<super::Msg>, generation: u32) -> Result<Self> {
        let c_uri = CString::new(uri)?;
        let context = Box::into_raw(Box::new(Context {
            events,
            generation,
            buffer: Vec::new(),
        }));

        let config = sys::esp_websocket_client_config_t {
            uri: c_uri.as_ptr(),
            disable_auto_reconnect: true,
            task_stack: 6144,
            buffer_size: 4096,
            network_timeout_ms: 10_000,
            ping_interval_sec: 20,
            pingpong_timeout_sec: 30,
            crt_bundle_attach: Some(sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        unsafe {
            let handle = sys::esp_websocket_client_init(&config);
            if handle.is_null() {
                drop(Box::from_raw(context));
                bail!("websocket client init failed");
            }
            sys::esp!(sys::esp_websocket_register_events(
                handle,
                sys::esp_websocket_event_id_t_WEBSOCKET_EVENT_ANY,
                Some(on_event),
                context.cast(),
            ))?;
            if let Err(e) = sys::esp!(sys::esp_websocket_client_start(handle)) {
                sys::esp_websocket_client_destroy(handle);
                drop(Box::from_raw(context));
                return Err(e.into());
            }
            Ok(Self {
                handle,
                context,
                _uri: c_uri,
            })
        }
    }

    pub fn send(&self, text: &str) -> Result<()> {
        let sent = unsafe {
            sys::esp_websocket_client_send_text(
                self.handle,
                text.as_ptr() as *const c_char,
                text.len() as i32,
                Duration::from_secs(5).as_millis() as u32,
            )
        };
        if sent < 0 {
            bail!("websocket send failed");
        }
        Ok(())
    }
}

impl Drop for WebSocket {
    fn drop(&mut self) {
        unsafe {
            sys::esp_websocket_client_stop(self.handle);
            sys::esp_websocket_client_destroy(self.handle);
            drop(Box::from_raw(self.context));
        }
    }
}

unsafe extern "C" fn on_event(
    arg: *mut c_void,
    _base: sys::esp_event_base_t,
    id: i32,
    data: *mut c_void,
) {
    let ctx = &mut *(arg as *mut Context);
    let send = |ctx: &Context, ev: WsEvent| {
        let _ = ctx.events.send(super::Msg::Ws(ctx.generation, ev));
    };
    match id {
        sys::esp_websocket_event_id_t_WEBSOCKET_EVENT_CONNECTED => send(ctx, WsEvent::Connected),
        sys::esp_websocket_event_id_t_WEBSOCKET_EVENT_DISCONNECTED
        | sys::esp_websocket_event_id_t_WEBSOCKET_EVENT_CLOSED
        | sys::esp_websocket_event_id_t_WEBSOCKET_EVENT_ERROR => send(ctx, WsEvent::Disconnected),
        sys::esp_websocket_event_id_t_WEBSOCKET_EVENT_DATA => {
            let d = &*(data as *const sys::esp_websocket_event_data_t);
            // 0 = continuation, 1 = text. Pings, pongs and close frames are
            // handled by the client itself.
            if d.op_code != 0 && d.op_code != 1 {
                return;
            }
            if d.op_code == 1 && d.payload_offset == 0 {
                ctx.buffer.clear();
            }
            if d.data_len > 0 {
                let chunk =
                    core::slice::from_raw_parts(d.data_ptr.cast::<u8>(), d.data_len as usize);
                ctx.buffer.extend_from_slice(chunk);
            }
            let frame_done = d.payload_offset + d.data_len >= d.payload_len;
            if frame_done && d.fin {
                let bytes = std::mem::take(&mut ctx.buffer);
                match String::from_utf8(bytes) {
                    Ok(text) => send(ctx, WsEvent::Text(text)),
                    Err(_) => log::warn!("dropping non-UTF-8 websocket message"),
                }
            }
        }
        _ => {}
    }
}
