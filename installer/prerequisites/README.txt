Yas prerequisites

The portable package includes MicrosoftEdgeWebview2Setup.exe, Microsoft's
signed Evergreen Bootstrapper. It is a small online setup helper, not a
complete offline copy of the WebView2 browser engine.

If Yas reports that WebView2 Runtime is missing:
1. Connect to the internet.
2. Run prerequisites\MicrosoftEdgeWebview2Setup.exe and follow Microsoft's setup.
3. Open Yas again after the Runtime installation completes.

Yas's interactive installer offers to run this helper only if WebView2
is missing and you explicitly choose Yes. An already-installed Runtime is
left in place. Silent Yas setup without a Runtime stops with instructions.

Yas includes its UI, logo, filter-list snapshots, and release Microsoft
Visual C++ runtime DLLs. Microsoft Edge WebView2 Runtime is shared and updated
by Microsoft. Dubai is used from Windows if installed; its font files are not
redistributed and the interface falls back to Segoe UI when it is unavailable.

Some sites, sign-in providers, extensions, DRM/media services, and enterprise
policies restrict embedded browsers. Supplying the Runtime does not guarantee
that every website or sign-in flow will work.

Official download and deployment documentation:
https://developer.microsoft.com/microsoft-edge/webview2/
https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution

WEBVIEW2_BOOTSTRAPPER.json records the original Microsoft URL, file version,
Microsoft signature status, and SHA-256 of the bundled helper. No Runtime was
installed or removed from the development computer as part of bundling it.
