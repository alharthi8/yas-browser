# Yas Browser 0.4.7 — experimental macOS preview

Apple Silicon (M1 or newer). Built with a macOS 13 deployment target; older macOS releases and Intel Macs have not been validated. Rust/Tao/Wry with Apple's WKWebView.

Includes tabs, navigation, search, bookmarks, local history, lazy session restore, sleeping eligible background tabs, Stealth views, file-save dialogs, zoom, printing, and signed whole-app updates. Stealth tabs are not saved in history or session restore and are not put to sleep. Sleeping ordinary tabs can lose unsaved page content and per-tab back/forward history. Closing a tab or the app can stop downloads.

This first Mac port does not include Windows extensions, password/autofill import, ad-filtering parity, screenshot/PDF export, resumable or tray downloads, full native menu/shortcut parity, or cross-device sync. It is not a full replacement for the Windows release yet. No comparative RAM/CPU percentage has been measured for this port.

## Install and update

Extract the preview ZIP and move Yas Browser.app to a writable Applications folder, such as ~/Applications. Do not run from inside the ZIP or a disk image. The preview has an ad-hoc development signature. It is **not Apple Developer ID signed or notarized**. macOS Gatekeeper may block opening it. See Apple's official guidance at https://support.apple.com/en-us/102445; do not disable Gatekeeper.

Open the app's menu panel and choose Check for app updates. Startup checks use the same channel. Updates require approval, verify the repository's pinned Ed25519 signature and the extracted bundle signature, and replace the complete .app bundle. Fully quit and reopen to use the update. User profile files are stored separately. This macOS preview channel does not install Windows assets or change the Windows latest-release channel.

Release automation requires native Rust tests, an actual WKWebView/frontend launch handshake, signature verification, and a signed full-bundle replacement/tamper-rejection test before publication. Real-world websites, downloads, native dialogs, and broad accessibility/keyboard behavior still need community testing. Report issues without personal browsing data.
