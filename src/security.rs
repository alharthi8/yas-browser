//! Browser security defaults, independent of the ad-blocking feature.
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings8;
use windows::core::{Interface, BOOL};
use wry::{WebView, WebViewExtWindows};

// Wry's application-oriented defaults also disable SmartScreen. A browser
// must retain its reputation checks. Preserve the other existing switches,
// including the existing autoplay policy, when replacing those defaults.
pub const ENVIRONMENT_ARGUMENTS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI --autoplay-policy=no-user-gesture-required";

/// Request and confirm the WebView setting. Windows settings can still
/// override this request; this does not attest to actual OS enforcement.
pub fn request_reputation_checks(view: &WebView) -> Result<(), String> {
    let result = (|| -> windows::core::Result<bool> {
        unsafe {
            let settings = view
                .webview()
                .Settings()?
                .cast::<ICoreWebView2Settings8>()?;
            settings.SetIsReputationCheckingRequired(true)?;
            let mut required = BOOL::default();
            settings.IsReputationCheckingRequired(&mut required)?;
            Ok(required.as_bool())
        }
    })();
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err("Yas could not confirm Microsoft website safety checks. Review Windows security settings and update Microsoft WebView2.".into()),
        Err(error) => Err(format!("Yas could not confirm Microsoft website safety checks: {error}. Review Windows security settings and update Microsoft WebView2.")),
    }
}
