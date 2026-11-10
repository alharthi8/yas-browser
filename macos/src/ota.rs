//! Signed GitHub updates. Only the public verification key is bundled.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
static BUSY: AtomicBool = AtomicBool::new(false);
static UPDATED: AtomicBool = AtomicBool::new(false);
const KEY: [u8; 32] = *include_bytes!("../../release.pub");
const CURRENT: &str = env!("CARGO_PKG_VERSION");
fn show(message: &str) {
    rfd::MessageDialog::new()
        .set_title("Yas Browser updates")
        .set_description(message)
        .show();
}
fn builder() -> self_update::backends::github::UpdateBuilder {
    let mut b = self_update::backends::github::Update::configure();
    b.repo_owner("alharthi8")
        .repo_name("yas-browser")
        .bin_name("yas-macos")
        .bundle_path_in_archive("Yas Browser.app")
        .bundle_install_path(bundle_path().expect("Launch the installed .app bundle"))
        .current_version(CURRENT)
        .target("aarch64-apple-darwin")
        .tag_prefix("macos-v")
        .asset_identifier("ota")
        .verifying_keys([KEY])
        .show_output(false)
        .show_download_progress(false)
        .no_confirm(true)
        .timeout(Duration::from_secs(120))
        .verify_binary(verify_bundle);
    b
}
fn verify_bundle(bundle: &std::path::Path) -> self_update::Result<()> {
    let status = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .status()
        .map_err(|e| self_update::Error::verification_rejected(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(self_update::Error::verification_rejected(
            "macOS bundle signature is invalid",
        ))
    }
}
fn bundle_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.ancestors().nth(3)?;
    if app.extension()?.to_str()? != "app" {
        return None;
    }
    Some(app.to_owned())
}
fn run(manual: bool) -> Result<(), String> {
    if bundle_path().is_none() {
        return Err("Open the app from its .app bundle to update it.".into());
    }
    let releases = builder()
        .build()
        .map_err(|e| e.to_string())?
        .get_newer_releases()
        .map_err(|e| e.to_string())?;
    if !releases.is_update_available().map_err(|e| e.to_string())? {
        if manual {
            show("You have the latest version.");
        }
        return Ok(());
    }
    let Some(release) = releases.latest() else {
        if manual {
            show("You have the latest macOS preview.");
        }
        return Ok(());
    };
    let version = release.version().to_owned();
    if !newer(CURRENT, &version) {
        return Err("Rejected an older or invalid update version.".into());
    }
    let expected = asset_name(&version);
    if !release.assets().iter().any(|a| a.name() == expected) {
        return Err("The signed update package is not available yet.".into());
    }
    let accept=rfd::MessageDialog::new().set_title("Yas Browser update available").set_description(format!("Version {version} is available. Download and apply the signed update? Your current session stays open. Fully quit and reopen when convenient to use the new version.\n\nUpdates replace the complete app bundle; your accounts, messages, bookmarks, and settings are kept.")).set_buttons(rfd::MessageButtons::YesNo).show();
    if accept != rfd::MessageDialogResult::Yes {
        return Ok(());
    }
    let mut b = builder();
    b.release_tag(format!("macos-v{version}"))
        .asset_matcher(move |assets| assets.iter().find(|a| a.name() == expected).cloned());
    let result = b
        .build()
        .map_err(|e| e.to_string())?
        .update()
        .map_err(|e| e.to_string())?;
    if result.is_updated() {
        UPDATED.store(true, Ordering::SeqCst);
        show("The signed update was applied. Finish your work, then fully quit and reopen the app. Use Quit from the application menu after saving your work.");
    }
    Ok(())
}
fn newer(current: &str, next: &str) -> bool {
    match (
        semver::Version::parse(current),
        semver::Version::parse(next),
    ) {
        (Ok(a), Ok(b)) => b > a,
        _ => false,
    }
}
fn asset_name(version: &str) -> String {
    format!("yas-browser-ota-{version}-aarch64-apple-darwin.zip")
}
pub fn check(manual: bool) {
    if UPDATED.load(Ordering::SeqCst) {
        if manual {
            show("An update is already applied. Fully quit and reopen the app to finish.");
        }
        return;
    }
    if BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    std::thread::spawn(move || {
        if let Err(error) = run(manual) {
            if manual {
                show(&format!("Update could not be completed: {error}\n\nYour current installation remains available. Try again later."));
            }
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}
pub fn diagnostic() -> bool {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--yas-verify-update") {
        let path = args.next().unwrap_or_default();
        let good = self_update::verify_signature(path, &[KEY]).is_ok();
        std::process::exit(if good { 0 } else { 2 });
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prevents_rollback() {
        assert!(newer("1.0.0", "1.0.1"));
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("1.0.0", "0.9.9"));
        assert!(!newer("1.0.0", "broken"));
    }
    #[test]
    fn key_is_not_empty() {
        assert!(KEY.iter().any(|b| *b != 0));
    }
    #[test]
    fn rejects_unsigned_archive() {
        let file = std::env::temp_dir().join(format!("yas-unsigned-{}.zip", std::process::id()));
        std::fs::write(&file, b"not a signed release").unwrap();
        assert!(self_update::verify_signature(&file, &[KEY]).is_err());
        let _ = std::fs::remove_file(file);
    }
}

#[cfg(test)]
mod release_tests;
