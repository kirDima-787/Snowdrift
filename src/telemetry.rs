use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Instant;

use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::core::{PCWSTR, w};

use crate::prefs;

/// Telemetry endpoint on the analytics worker.
/// When deployed to Cloudflare Workers, anonymous usage metrics and session duration are reported.
/// If empty, unreachable, or offline, all requests silently fail with no impact on the application.
pub const ENDPOINT: &str = "https://snowdrift.pages.dev/api/telemetry";

static START_TIME: OnceLock<Instant> = OnceLock::new();
static LAST_HEARTBEAT_SECS: AtomicU64 = AtomicU64::new(0);

struct Handle(*mut std::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub fn start() {
    START_TIME.get_or_init(Instant::now);
    send_event("start", 0);
}

pub fn heartbeat() {
    if let Some(start) = START_TIME.get() {
        let elapsed_mins = start.elapsed().as_secs() / 60;
        LAST_HEARTBEAT_SECS.store(start.elapsed().as_secs(), Ordering::Relaxed);
        send_event("heartbeat", elapsed_mins);
    }
}

pub fn stop() {
    if let Some(start) = START_TIME.get() {
        let elapsed_mins = start.elapsed().as_secs() / 60;
        send_event("stop", elapsed_mins);
    }
}

fn send_event(event: &'static str, duration_mins: u64) {
    if ENDPOINT.is_empty() {
        return;
    }

    let uid = prefs::install_id();
    let version = env!("CARGO_PKG_VERSION");
    let payload = format!(
        r#"{{"uid":"{uid}","event":"{event}","version":"{version}","os":"windows","duration_mins":{duration_mins}}}"#
    );

    // Completely detached thread with 2s timeout: silent, non-blocking, zero impact on UI or exit
    thread::spawn(move || {
        let _ = post_json(ENDPOINT, &payload);
    });
}

fn post_json(url: &str, json: &str) -> Result<(), ()> {
    let (_scheme, rest, is_https) = if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http", rest, false)
    } else {
        return Err(());
    };

    let slash = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..slash];
    let path = &rest[slash..];
    let path = if path.is_empty() { "/" } else { path };

    let host_w: Vec<u16> = host.encode_utf16().chain([0]).collect();
    let path_w: Vec<u16> = path.encode_utf16().chain([0]).collect();

    let session = unsafe {
        let handle = WinHttpOpen(
            w!("Snowdrift"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if handle.is_null() {
            return Err(());
        }
        Handle(handle)
    };

    unsafe {
        // Resolve: 2s, Connect: 2s, Send: 2s, Receive: 2s
        let _ = WinHttpSetTimeouts(session.0, 2000, 2000, 2000, 2000);
    }

    let connection = unsafe {
        let handle = WinHttpConnect(
            session.0,
            PCWSTR(host_w.as_ptr()),
            if is_https { 443 } else { 80 },
            0,
        );
        if handle.is_null() {
            return Err(());
        }
        Handle(handle)
    };

    let flags = if is_https {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };

    let request = unsafe {
        let handle = WinHttpOpenRequest(
            connection.0,
            w!("POST"),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        );
        if handle.is_null() {
            return Err(());
        }
        Handle(handle)
    };

    let headers: Vec<u16> = "Content-Type: application/json\r\nUser-Agent: Snowdrift\r\n"
        .encode_utf16()
        .collect();

    let body_bytes = json.as_bytes();

    unsafe {
        WinHttpSendRequest(
            request.0,
            Some(&headers),
            Some(body_bytes.as_ptr().cast()),
            body_bytes.len() as u32,
            body_bytes.len() as u32,
            0,
        )
        .map_err(|_| ())?;

        let _ = WinHttpReceiveResponse(request.0, std::ptr::null_mut());
    }

    Ok(())
}
