//! Full-document exports. All WebView2 calls stay on its UI thread; only
//! decoding and file I/O move to workers after COM callbacks complete.
use crate::Msg;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tao::event_loop::EventLoopProxy;
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    PrintToPdfCompletedHandler,
};
use windows::{
    core::{Interface, HSTRING, PWSTR},
    Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS},
};
use wry::{WebView, WebViewExtWindows};

const MAX_SIDE: u32 = 32_767;
const MAX_PIXELS: u64 = 128_000_000;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Dimensions {
    width: u32,
    height: u32,
}

fn checked_dimensions(width: f64, height: f64) -> Result<Dimensions, String> {
    if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
        return Err(
            "The page did not report valid document dimensions. Try again after it loads.".into(),
        );
    }
    let (width, height) = (width.ceil(), height.ceil());
    if width > MAX_SIDE as f64 || height > MAX_SIDE as f64 || width * height > MAX_PIXELS as f64 {
        return Err(format!("This page is too large for one PNG (maximum {MAX_SIDE} pixels per side and 128 megapixels). Export it as PDF instead."));
    }
    Ok(Dimensions {
        width: width as u32,
        height: height as u32,
    })
}

fn document_dimensions(metrics: &str) -> Result<Dimensions, String> {
    let v: Value =
        serde_json::from_str(metrics).map_err(|_| "WebView2 returned invalid page metrics")?;
    let content = v
        .get("cssContentSize")
        .or_else(|| v.get("contentSize"))
        .ok_or("WebView2 did not return the full document dimensions")?;
    checked_dimensions(
        content["width"].as_f64().ok_or("Missing document width")?,
        content["height"]
            .as_f64()
            .ok_or("Missing document height")?,
    )
}

fn current_url(web: &ICoreWebView2) -> Result<String, String> {
    unsafe {
        let mut raw = PWSTR::null();
        web.Source(&mut raw).map_err(|e| e.to_string())?;
        Ok(take_pwstr(raw))
    }
}

fn export_url(view: &WebView, path: &Path) -> Result<String, String> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err("Choose an absolute destination file path.".into());
    }
    let url = view.url().map_err(|e| e.to_string())?;
    if !url::Url::parse(&url)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
    {
        return Err("Open a webpage before exporting it.".into());
    }
    Ok(url)
}

fn unchanged(web: &ICoreWebView2, original: &str) -> Result<(), String> {
    if current_url(web)? != original {
        return Err("The page changed during export. Try exporting the new page again.".into());
    }
    Ok(())
}

fn announce(proxy: &EventLoopProxy<Msg>, result: Result<(), String>, kind: &str) {
    let message = match result {
        Ok(()) => format!("Full-page {kind} saved"),
        Err(e) => format!("{kind} export failed: {e}"),
    };
    let _ = proxy.send_event(Msg::Toast(message));
}

/// A unique sibling avoids overwriting an unrelated `page.tmp` file. On failure
/// the selected destination is left intact; only this operation's temporary file
/// is removed. Native PDF printing also writes into this temporary destination.
struct PendingFile(PathBuf);
impl PendingFile {
    fn create(destination: &Path, extension: &str) -> Result<(Self, File), String> {
        let name = destination
            .file_name()
            .ok_or("Destination has no filename")?;
        let parent = destination
            .parent()
            .ok_or("Destination has no parent directory")?;
        for _ in 0..8 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos();
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut temporary_name = std::ffi::OsString::from(".");
            temporary_name.push(name);
            temporary_name.push(format!(
                ".yas-{}-{stamp}-{sequence}.tmp.{extension}",
                std::process::id()
            ));
            let candidate = parent.join(temporary_name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => return Ok((Self(candidate), file)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Could not create a unique temporary export file".into())
    }
    fn commit(self, destination: &Path) -> Result<(), String> {
        if destination.exists() {
            unsafe {
                ReplaceFileW(
                    &HSTRING::from(destination.as_os_str()),
                    &HSTRING::from(self.0.as_os_str()),
                    None,
                    REPLACE_FILE_FLAGS(0),
                    None,
                    None,
                )
                .map_err(|e| e.to_string())
            }
        } else {
            fs::rename(&self.0, destination).map_err(|e| e.to_string())
        }
    }
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn validate_png(bytes: &[u8], requested: Dimensions) -> Result<(), String> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err("WebView2 did not return a valid PNG image".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    checked_dimensions(width as f64, height as f64)?;
    if width.saturating_add(1) < requested.width || height.saturating_add(1) < requested.height {
        return Err(
            "WebView2 returned a cropped image; no screenshot was saved. Try PDF export instead."
                .into(),
        );
    }
    Ok(())
}

fn write_png(path: &Path, payload: &str, dimensions: Dimensions) -> Result<(), String> {
    let result: Value = serde_json::from_str(payload).map_err(|_| "Invalid screenshot response")?;
    let encoded = result["data"]
        .as_str()
        .ok_or("WebView2 returned no screenshot data")?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|e| format!("Invalid screenshot encoding: {e}"))?;
    validate_png(&bytes, dimensions)?;
    let (pending, mut file) = PendingFile::create(path, "png")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    pending.commit(path)
}

fn request_screenshot(
    web: ICoreWebView2,
    path: PathBuf,
    original_url: String,
    dimensions: Dimensions,
    dpr: f64,
    proxy: EventLoopProxy<Msg>,
) -> Result<(), String> {
    if !dpr.is_finite() || !(0.25..=8.).contains(&dpr) {
        return Err("WebView2 returned an unsupported display scale".into());
    }
    unchanged(&web, &original_url)?;
    // CDP's clip is expressed in CSS pixels, while its output uses device pixels.
    // Normalize by DPR so a 15,000px page remains 15,000px on a 225% display.
    let parameters = json!({
        "format":"png", "fromSurface":true, "captureBeyondViewport":true,
        "clip":{"x":0,"y":0,"width":dimensions.width,"height":dimensions.height,"scale":1.0/dpr}
    })
    .to_string();
    let callback_web = web.clone();
    unsafe {
        web.CallDevToolsProtocolMethod(
            &HSTRING::from("Page.captureScreenshot"),
            &HSTRING::from(parameters),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                move |status, payload| {
                    if let Err(e) = status {
                        announce(&proxy, Err(e.to_string()), "PNG");
                        return Ok(());
                    }
                    if let Err(e) = unchanged(&callback_web, &original_url) {
                        announce(&proxy, Err(e), "PNG");
                        return Ok(());
                    }
                    std::thread::spawn(move || {
                        announce(&proxy, write_png(&path, &payload, dimensions), "PNG");
                    });
                    Ok(())
                },
            )),
        )
        .map_err(|e| e.to_string())
    }
}

/// Save the complete currently rendered document as one PNG, excluding the
/// browser toolbar. Oversized pages fail explicitly rather than being truncated.
pub fn save_png(view: &WebView, path: PathBuf, proxy: EventLoopProxy<Msg>) -> Result<(), String> {
    let original_url = export_url(view, &path)?;
    let web = view.webview();
    let callback_web = web.clone();
    unsafe {
        web.CallDevToolsProtocolMethod(
            &HSTRING::from("Page.getLayoutMetrics"), &HSTRING::from("{}"),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, metrics| {
                let dimensions = match status.map_err(|e| e.to_string()).and_then(|_| document_dimensions(&metrics)) {
                    Ok(value) => value,
                    Err(e) => { announce(&proxy, Err(e), "PNG"); return Ok(()); }
                };
                if let Err(e) = unchanged(&callback_web, &original_url) {
                    announce(&proxy, Err(e), "PNG"); return Ok(());
                }
                let evaluate_web = callback_web.clone();
                let error_proxy = proxy.clone();
                let started = callback_web.CallDevToolsProtocolMethod(
                    &HSTRING::from("Runtime.evaluate"),
                    &HSTRING::from(json!({"expression":"window.devicePixelRatio || 1","returnByValue":true}).to_string()),
                    &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, data| {
                        let result = status.map_err(|e| e.to_string()).and_then(|_| {
                            let v: Value = serde_json::from_str(&data).map_err(|_| "Invalid display-scale response")?;
                            let dpr = v["result"]["value"].as_f64().ok_or("Missing display scale")?;
                            request_screenshot(evaluate_web, path, original_url, dimensions, dpr, proxy.clone())
                        });
                        if let Err(e) = result { announce(&proxy, Err(e), "PNG"); }
                        Ok(())
                    })),
                );
                if let Err(e) = started { announce(&error_proxy, Err(e.to_string()), "PNG"); }
                Ok(())
            })),
        ).map_err(|e| format!("PNG capture requires an up-to-date WebView2 runtime: {e}"))
    }
}

/// Print the complete document into a paginated PDF, using the site's print
/// layout and background colors. Never prints only the current selection.
pub fn save_pdf(view: &WebView, path: PathBuf, proxy: EventLoopProxy<Msg>) -> Result<(), String> {
    let original_url = export_url(view, &path)?;
    let web = view.webview();
    unsafe {
        let printable = web
            .cast::<ICoreWebView2_7>()
            .map_err(|e| format!("PDF export requires an up-to-date WebView2 runtime: {e}"))?;
        let environment = view
            .environment()
            .cast::<ICoreWebView2Environment6>()
            .map_err(|e| e.to_string())?;
        let settings = environment
            .CreatePrintSettings()
            .map_err(|e| e.to_string())?;
        settings
            .SetShouldPrintBackgrounds(true)
            .map_err(|e| e.to_string())?;
        settings
            .SetShouldPrintSelectionOnly(false)
            .map_err(|e| e.to_string())?;
        settings
            .SetShouldPrintHeaderAndFooter(false)
            .map_err(|e| e.to_string())?;
        let (pending, file) = PendingFile::create(&path, "pdf")?;
        drop(file);
        let temporary_path = HSTRING::from(pending.0.as_os_str());
        let result = printable.PrintToPdf(
            &temporary_path,
            &settings,
            &PrintToPdfCompletedHandler::create(Box::new(move |status, succeeded| {
                let ready = status.map_err(|e| e.to_string()).and_then(|_| {
                    if !succeeded {
                        return Err("The page could not be printed to PDF".into());
                    }
                    unchanged(&web, &original_url)
                });
                if let Err(e) = ready {
                    announce(&proxy, Err(e), "PDF");
                    return Ok(());
                }
                std::thread::spawn(move || {
                    let result = (|| {
                        let mut file = File::open(&pending.0).map_err(|e| e.to_string())?;
                        let mut signature = [0u8; 5];
                        file.read_exact(&mut signature).map_err(|e| e.to_string())?;
                        drop(file);
                        if &signature != b"%PDF-" {
                            return Err("WebView2 produced an invalid PDF".into());
                        }
                        pending.commit(&path)
                    })();
                    announce(&proxy, result, "PDF");
                });
                Ok(())
            })),
        );
        result.map_err(|e| format!("Could not start PDF export: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_document_metrics_use_css_dimensions() {
        let metrics = r#"{"cssContentSize":{"width":1200.2,"height":15000.1},"contentSize":{"width":2700,"height":33750}}"#;
        assert_eq!(
            document_dimensions(metrics).unwrap(),
            Dimensions {
                width: 1201,
                height: 15001
            }
        );
        assert_eq!(
            document_dimensions(r#"{"contentSize":{"width":800,"height":1200}}"#).unwrap(),
            Dimensions {
                width: 800,
                height: 1200
            }
        );
    }
    #[test]
    fn rejects_invalid_or_oversized_documents_instead_of_cropping() {
        assert!(checked_dimensions(f64::NAN, 100.).is_err());
        assert!(checked_dimensions(800., 0.).is_err());
        assert!(checked_dimensions(800., 32768.).is_err());
        assert!(checked_dimensions(20000., 20000.).is_err());
        assert!(checked_dimensions(1200., 15000.).is_ok());
    }
    #[test]
    fn rejects_viewport_only_png_for_tall_document() {
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend(1200u32.to_be_bytes());
        header.extend(800u32.to_be_bytes());
        assert!(validate_png(
            &header,
            Dimensions {
                width: 1200,
                height: 15000
            }
        )
        .is_err());
    }
}
