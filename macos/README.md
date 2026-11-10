# Yas Browser macOS preview

Native Rust/Tao/Wry application using Apple's WKWebView. This first Apple Silicon preview is isolated from the Windows implementation.

Included: tabs, navigation, search, bookmarks, local history, lazy session restoration, sleeping eligible background pages, isolated Stealth views, file-save dialogs for downloads, zoom, print, and an in-app signed whole-bundle updater. Only the local shell has an IPC handler; websites cannot call native commands. Only HTTP/HTTPS navigation is permitted.

Not yet included: Windows extension support, Windows password/autofill import, ad-filtering parity, screenshot/PDF export, resumable downloads, background tray downloads, full native menu/shortcut parity, and cross-device sync. Private views are excluded from session/history persistence. Closing a tab or the app can stop a download. Sleeping tabs can lose unsaved page contents. This is not feature parity with the Windows release.

Build on an Apple Silicon Mac with Rust and Xcode Command Line Tools: `bash build.sh`. The script runs native tests, builds the app, verifies its local signature, and creates a ZIP in dist/. Without YAS_CODESIGN_IDENTITY it uses ad-hoc development signing. This is not Apple Developer ID signing or notarization and must be labeled as experimental rather than advertised as a notarized public release.

The app uses a separate macOS profile under the user's Application Support directory. For launch QA, use `--user-data-dir /absolute/path/to/a/disposable/profile` to avoid touching the normal profile. Native launch, download, window/shortcut, and signed-upgrade checks still require a signed-in desktop session.

Updates use the app's existing pinned public key and version-bound signed ZIP packages containing the full Yas Browser.app. macOS releases use `macos-vVERSION` tags and a separate updater selection path, leaving the Windows latest-release channel intact. A complete `.app` bundle and valid macOS bundle signature are checked before replacement. A proper Developer ID/notarization flow must be configured before seamless public distribution is claimed.
