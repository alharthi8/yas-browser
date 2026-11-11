# Yas Browser 1.0.0

The first numbered Windows launch of Yas Browser, designed and built by Appana Technologies in the UAE. Rust/Tao/Wry desktop core with Microsoft's WebView2 rendering engine.

## Windows download

Windows 10 22H2 / Windows 11, x64. Choose **Yas-1.0.0-Setup-x64.exe** for the guided installer or **Yas-1.0.0-Windows-x64.zip** for portable use. Extract the complete portable ZIP; keep its runtime and helper folders together. WebView2 is required; the installer offers Microsoft's bootstrapper if needed. Installation does not change your default browser. The installer and browser are not Authenticode publisher-signed, so Windows policy may warn or block them.

Includes tabs, bookmarks, history, session restore, saved tab titles, background-tab memory management, Stealth browsing, ad/tracker filtering, downloads, printing, page capture, local import and autofill tools, appearance/search preferences, and website notification controls. The local redb workspace retains DPAPI protection for the password vault and download records. Legacy migration validates the inputs and preserves recovery files.

Local AI is optional and explicitly initiated. The package contains the reviewed CPU runtime and its notices; the separate model is downloaded only through setup. AI generation can use substantial CPU and RAM while active. It has no browser-action tools. No paid Google/Microsoft cloud account integration or cross-device sync is included. Some sites restrict embedded browser engines; no universal provider-sign-in compatibility is claimed.

## Updates

The installer enables signed GitHub updates. Use **Settings → Check for app updates**, approve an available update, then fully quit and reopen when convenient. The active session stays open. Only the public verification key ships with the app.

In-app updates replace the executable. Install the full 1.0.0 package once when upgrading from a pre-1.0 preview to obtain the local AI runtime, automation helper, and current license notices. Later changes that require new runtime files will also need a full installer. User browsing data stays outside the application directory; keep backups before upgrading early development profiles. Older apps must not be used to write a newer profile database.

## Limits

Sleeping tabs can lose unsaved page content. Stealth excludes private history/session entries; bookmarks and downloaded files remain, and websites or networks can still observe activity. Closing Yas through its tray Exit command stops active downloads. Not all Chromium extensions work. Native Web Push and cross-device browser-account sync are not provided. No blanket percentage reduction in RAM, CPU, battery use or device load is claimed.

The Mac port is separate and does not have Windows feature parity. Its availability and limitations are listed on yasapps.com.

Release automation requires Rust tests, an actual WebView2 launch/navigation/interaction check, exact runtime inventory/hash checks, and signed update acceptance with damaged-package rejection. Broader Windows regression checks use synthetic data in isolated profiles. Historical 0.4.x reports do not establish test results for this release.
