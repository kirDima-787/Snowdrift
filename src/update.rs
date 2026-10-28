use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_CONTENT_LENGTH, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::version::{field, newer};

const REPOSITORY: &str = "kirDima-787/Snowdrift";
const INSTALLER: &str = "Snowdrift-Setup.exe";

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub url: String,
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub fn check() -> Result<Option<Release>, String> {
    let body = fetch_text(&format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"))?;
    let tag = field(&body, "tag_name").ok_or("The latest release has no tag")?;
    let version = tag.trim_start_matches('v');
    if !body.contains(&format!("\"{INSTALLER}\"")) || !newer(version, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let url = format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{INSTALLER}");
    Ok(Some(Release { version: version.to_string(), url }))
}

pub fn download_installer(url: &str, progress: Option<Box<dyn Fn(u32, u32) + Send>>) -> Result<PathBuf, String> {
    let temp_file = std::env::temp_dir().join(INSTALLER);
    let _ = std::fs::remove_file(&temp_file);
    download_file(url, &temp_file, &progress)?;
    Ok(temp_file)
}

pub fn run_installer(path: &Path) -> Result<(), String> {
    let path_w: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let arg_w: Vec<u16> = "--update".encode_utf16().chain([0]).collect();
    unsafe {
        let res = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(path_w.as_ptr()),
            PCWSTR(arg_w.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if res.0 as usize <= 32 {
            return Err("Failed to launch installer process".into());
        }
    }
    Ok(())
}

fn parse_url(url: &str) -> Result<(String, String, bool), String> {
    let (_scheme, rest, is_https) = if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http", rest, false)
    } else {
        return Err("Invalid URL scheme".into());
    };
    let slash = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..slash];
    let path = &rest[slash..];
    let path = if path.is_empty() { "/" } else { path };
    Ok((host.to_string(), path.to_string(), is_https))
}

fn fetch_text(url: &str) -> Result<String, String> {
    let bytes = fetch_bytes(url)?;
    String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8 response: {e}"))
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let mut current_url = url.to_string();
    let mut redirects = 0;

    let session = unsafe {
        let handle = WinHttpOpen(w!("Snowdrift"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if handle.is_null() {
            return Err("Failed to open WinHttp session".into());
        }
        Handle(handle)
    };

    unsafe {
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
    }

    while redirects < 5 {
        let (host, path, is_https) = parse_url(&current_url)?;
        let host_w: Vec<u16> = host.encode_utf16().chain([0]).collect();
        let path_w: Vec<u16> = path.encode_utf16().chain([0]).collect();

        let connection = unsafe {
            let handle = WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), if is_https { 443 } else { 80 }, 0);
            if handle.is_null() {
                return Err(format!("Failed to connect to {host}"));
            }
            Handle(handle)
        };

        let flags = if is_https { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = unsafe {
            let handle = WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(path_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            );
            if handle.is_null() {
                return Err("Failed to open WinHttp request".into());
            }
            Handle(handle)
        };

        let headers: Vec<u16> = "User-Agent: Snowdrift\r\nAccept: application/vnd.github+json, */*\r\n".encode_utf16().collect();
        unsafe {
            WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).map_err(|e| e.message())?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| e.message())?;
        }

        let mut status = 0u32;
        let mut size = 4u32;
        unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&raw mut status).cast()),
                &mut size,
                std::ptr::null_mut(),
            )
            .map_err(|e| e.message())?;
        }

        if (301..=308).contains(&status) {
            let mut loc_size = 0u32;
            unsafe {
                let _ = WinHttpQueryHeaders(request.0, WINHTTP_QUERY_LOCATION, PCWSTR::null(), None, &mut loc_size, std::ptr::null_mut());
            }
            if loc_size > 0 {
                let mut buf = vec![0u16; (loc_size as usize) / 2];
                unsafe {
                    WinHttpQueryHeaders(
                        request.0,
                        WINHTTP_QUERY_LOCATION,
                        PCWSTR::null(),
                        Some(buf.as_mut_ptr() as *mut _),
                        &mut loc_size,
                        std::ptr::null_mut(),
                    )
                    .map_err(|e| e.message())?;
                }
                let loc = String::from_utf16_lossy(&buf);
                current_url = loc.trim_matches('\0').trim().to_string();
                redirects += 1;
                continue;
            }
        }

        if status != 200 {
            return Err(format!("Server returned HTTP status {status}"));
        }

        let mut body = Vec::new();
        let mut buffer = vec![0u8; 32 * 1024];
        loop {
            let mut read = 0u32;
            unsafe {
                WinHttpReadData(request.0, buffer.as_mut_ptr() as *mut _, buffer.len() as u32, &mut read)
                    .map_err(|e| e.message())?;
            }
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buffer[..read as usize]);
        }
        return Ok(body);
    }

    Err("Too many redirects".into())
}

fn download_file(url: &str, target_path: &Path, progress: &Option<Box<dyn Fn(u32, u32) + Send>>) -> Result<(), String> {
    let mut current_url = url.to_string();
    let mut redirects = 0;

    let session = unsafe {
        let handle = WinHttpOpen(w!("Snowdrift"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if handle.is_null() {
            return Err("Failed to open WinHttp session".into());
        }
        Handle(handle)
    };

    unsafe {
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000);
    }

    while redirects < 5 {
        let (host, path, is_https) = parse_url(&current_url)?;
        let host_w: Vec<u16> = host.encode_utf16().chain([0]).collect();
        let path_w: Vec<u16> = path.encode_utf16().chain([0]).collect();

        let connection = unsafe {
            let handle = WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), if is_https { 443 } else { 80 }, 0);
            if handle.is_null() {
                return Err(format!("Failed to connect to {host}"));
            }
            Handle(handle)
        };

        let flags = if is_https { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = unsafe {
            let handle = WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(path_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            );
            if handle.is_null() {
                return Err("Failed to prepare request".into());
            }
            Handle(handle)
        };

        let headers: Vec<u16> = "User-Agent: Snowdrift\r\nAccept: */*\r\n".encode_utf16().collect();
        unsafe {
            WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).map_err(|e| e.message())?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| e.message())?;
        }

        let mut status = 0u32;
        let mut size = 4u32;
        unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&raw mut status).cast()),
                &mut size,
                std::ptr::null_mut(),
            )
            .map_err(|e| e.message())?;
        }

        if (301..=308).contains(&status) {
            let mut loc_size = 0u32;
            unsafe {
                let _ = WinHttpQueryHeaders(request.0, WINHTTP_QUERY_LOCATION, PCWSTR::null(), None, &mut loc_size, std::ptr::null_mut());
            }
            if loc_size > 0 {
                let mut buf = vec![0u16; (loc_size as usize) / 2];
                unsafe {
                    WinHttpQueryHeaders(
                        request.0,
                        WINHTTP_QUERY_LOCATION,
                        PCWSTR::null(),
                        Some(buf.as_mut_ptr() as *mut _),
                        &mut loc_size,
                        std::ptr::null_mut(),
                    )
                    .map_err(|e| e.message())?;
                }
                let loc = String::from_utf16_lossy(&buf);
                current_url = loc.trim_matches('\0').trim().to_string();
                redirects += 1;
                continue;
            }
        }

        if status != 200 {
            return Err(format!("Download failed with status {status}"));
        }

        let mut content_length = 0u32;
        let mut cl_size = 4u32;
        unsafe {
            let _ = WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&raw mut content_length).cast()),
                &mut cl_size,
                std::ptr::null_mut(),
            );
        }

        let mut file = File::create(target_path).map_err(|e| format!("Failed to create destination file: {e}"))?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut total_read = 0u32;
        loop {
            let mut read = 0u32;
            unsafe {
                WinHttpReadData(request.0, buffer.as_mut_ptr() as *mut _, buffer.len() as u32, &mut read)
                    .map_err(|e| e.message())?;
            }
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read as usize])
                .map_err(|e| format!("Failed to write downloaded data: {e}"))?;
            total_read += read;
            if let Some(cb) = progress {
                cb(total_read, content_length);
            }
        }
        return Ok(());
    }

    Err("Too many redirects".into())
}
