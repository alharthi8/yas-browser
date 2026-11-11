# Yas Browser — Windows browser preview

Yas Browser is a Rust desktop browser using Tao and Wry with the installed Microsoft Edge WebView2 engine. Version 0.4.9 uses Dubai, modern blue branding, a UAE-inspired falcon icon, and a Chrome-style tab strip with integrated window controls. It is a browser preview with compatibility and feature limits described below.

## Run

Run `Yas-0.4.9-Setup-x64.exe` for a per-user Windows 10/11 x64 installation with Start menu shortcut and uninstaller. A desktop shortcut is optional. Upgrades and uninstall preserve your browser data. When WebView2 is missing, setup offers to run the bundled Microsoft-signed Evergreen bootstrapper. This downloads the engine and needs internet; it is not a full offline runtime. The installer asks first and does not change your default browser.

Before updating or uninstalling, let live downloads finish or explicitly choose Exit from Yas's tray menu. The window's X can leave Yas running in the background. Exit stops unfinished native transfers; saved history does not make those operations resumable after the process ends.

For portable use, extract the portable ZIP and keep `Yas.exe`, `vcruntime140.dll`, `vcruntime140_1.dll`, and the `ai-runtime` directory together. No Rust installation is needed. Dubai must be installed to display the intended typography; font files are not bundled. The installer and portable executable are not publisher-signed.

The release now launches on the development computer after the user changed their Smart App Control setting. Yas remains unsigned; other computers may block it according to their policy. See `RELEASE_NOTES.md` for current validation results.

## Included

- Multiple tabs, address/search bar, back/forward, reload, stop, bookmarks, history, downloads with save dialog, progress, Pause, Continue, and Cancel, print, zoom, developer tools, and keyboard shortcuts.
- Local address-bar suggestions search all imported browsing history, bookmarks, and saved website origins. Type at least two characters; click a result or use Arrow keys and Enter. Plain Enter keeps web search. Suggestions contain no passwords or usernames. Stealth excludes regular history while retaining saved bookmarks/sites.
- The address bar follows links, redirects, back/forward, and History API or fragment URL changes. Focus alone does not freeze its value; an unsubmitted draft is kept only while you are editing.
- Passwords & autofill includes search by website, saved label, or username, result counts, and Open site actions. Changing the filter immediately clears any revealed password. Username-first HTTPS forms can fill the username before the password step; choose Fill again after continuing. Fill messages report actual matching fields rather than claiming success when nothing matched. Imported generic values remain under Saved form entries; structured address profiles are separate.
- The Downloads manager has filter chips, per-file byte/percentage progress, active speed/time estimates, Pause, Continue when supported, Cancel, and completed-file Open/Show folder actions. Active or paused native jobs keep their required source view loaded. Closing the source tab requests cancellation. With live regular downloads, the window's X hides Yas to the tray so the same process can keep working; reopening it does not restart those jobs. Servers may require a transfer to start again when they cannot serve byte ranges.
- Regular download history is stored as a Windows-DPAPI-encrypted record in `yas.redb` for the current Windows account. The history restores metadata and completed-file actions, not live native operations. An unfinished record from a process or Windows restart is shown as Interrupted and cannot automatically Continue; start a new download from the website. Downloaded files themselves remain in the locations you chose and are not encrypted by this history feature. Private transfer records are never included in persisted history.
- Session restore retains every saved regular tab as a lightweight entry and loads pages lazily. The normal budget retains up to five recent ordinary website views. Memory pressure, a failed memory query, or repeated failure to confirm native suspension reduces that budget to two. Unprotected background views expire after 120 seconds. Playing-audio pages and active or paused download owners are protected from automatic trimming, expiry, and suspension, so total residency can exceed the ordinary budget. A retained view keeps its live document; an evicted or explicitly sleeping tab reloads and can lose unsaved forms, playback, and per-tab navigation history.
- Hidden background pages request WebView2 suspension to pause timers and animations when the engine permits it. Yas checks the native result and uses a bounded fallback when suspension repeatedly fails. Library data is sent to the interface only when it changes; tab/list rendering avoids replacing unchanged content. Settings writes are coalesced at two-second intervals and flushed on normal exit. A crash can lose the most recent two seconds of settings changes.
- Full-page PNG export from the camera button or Settings, plus paginated PDF export from Settings. PNG includes the complete currently rendered document at CSS-pixel dimensions, up to 32,767 pixels per side and 128 megapixels. Larger pages report an error and suggest PDF. Infinite feeds and lazy content must be loaded first; PDF uses the website's print layout.
- Stealth mode (Ctrl+Shift+N) uses an isolated in-private WebView2 profile. New tabs inherit Stealth until you choose regular browsing. Stealth pages stay out of local history/session restore. Cookies/storage persist while Stealth tabs are sleeping, and clear after the last Stealth tab closes. Stealth transfers do not keep the window-close background session alive by default. If you explicitly enable private background downloads, their Stealth session and cookies remain alive until those private jobs finish or you explicitly end the session. Private transfer history is still never persisted. Bookmarks and downloaded files remain. Websites and the network can still see activity.
- Built-in adblock-rust request filtering, EasyList and EasyPrivacy snapshots, standard CSS cosmetic hiding, generic CSS matching from page classes/IDs, site exceptions, custom ABP-syntax rules, blocked counts, and manual filter updates. Lists refresh on launch if the cached copy is older than four days.
- Filtering reuses each view's top-level host while its document context is unchanged; current rules and site exceptions still apply to each request. An embedded compiled engine targets initialization of the exact unchanged bundled filter snapshots. Metadata and input checks gate that path; modified, updated, custom, or invalid inputs use the normal text loader. Cache use and startup improvements require release-specific evidence rather than an assumption from the source code.
- Local password saving offered by WebView2. The Yas vault supports manual logins, password CSV imports, exact HTTPS-origin credential filling, and address filling through standard autocomplete fields. Imported credentials, address profiles, and saved form values are encrypted with Windows DPAPI for the current Windows account. HTTP and Android-app credentials are preserved for manual lookup; only matching HTTPS website credentials can fill. Passwords are revealed only with the Show action and hidden again after 15 seconds. Saved form values fill a matching visible field only after an explicit action.
- Tabbit migration preserves bookmark folders, all accepted history URLs and timestamps, saved form entries, exported passwords, and a list of imported open tabs. Imports merge with existing data and create backups before writes. Imported tabs are listed in Settings for opening individually. Signed-in cookies, payment cards, Microsoft sync, and extensions do not transfer automatically.
- Unpacked Chromium extensions can be loaded and removed through Extensions. Open a regular website first. Keep the unpacked folder in place. The command `Yas.exe --load-extension "C:\path\to\extension"` also loads one. Only the regular website profile receives installed extensions; the UI and Stealth profiles do not.
- Website notifications ask for Allow or Block through Yas. Regular choices are saved locally and can be reset in Settings. Stealth choices are separate, kept in memory, and end when the last Stealth tab closes. Notifications require the originating website to remain loaded; sleeping the tab, closing it, or exiting Yas stops that page's ability to notify. Service-worker Web Push is not supported. Windows notification settings and Do Not Disturb can prevent banners.
- Yas requests Microsoft Defender SmartScreen reputation checks for websites and downloads. Windows security settings can override the request, even when the WebView2 setting reads back as enabled. The request alone does not prove that a particular website or download was checked or blocked. Reputation checks can send website and download information to Microsoft; see the [Microsoft privacy statement](https://privacy.microsoft.com/privacystatement).

## Limits

Microsoft account password sync, Chrome Web Store installation, extension toolbar popups, native messaging integrations, password generation, Windows Hello unlock, payment autofill, a browser updater, default-browser registration, accessibility certification, and signed installers are not implemented. Extension compatibility varies. Some providers block sign-in through embedded engines. Advanced adblock scriptlets, procedural filters, redirect resources, removeparam rewriting, and guaranteed video-ad removal are not implemented. No comparative claim of “best blocker” or “lowest memory” has been verified. Performance and memory depend on pages and extensions.

The UI is a separate profile with a strict navigation allowlist and a content security policy. Remote pages receive no Yas privileged IPC bridge. WebView2 profiles share the underlying environment to avoid duplicate browser/GPU processes. The app does not read or decrypt your existing Edge profile directly.

## Preferences and local AI

The public installer and portable download include an optional agent skill under `assistant/yas-browser`. Install that folder in your compatible agent's skill directory. Its Node.js22+ launcher opens a separate Yas Browser automation profile with a loopback debugging endpoint; ordinary browsing does not enable automation. Close the automation session when finished. Local software can control that session while the endpoint is open. The skill does not expose saved passwords or cookies, and account authentication remains a user action.

Open Settings for Light, Dark or System appearance and your default search engine: Bing, Google, DuckDuckGo or Brave Search. Appearance changes the Yas interface; websites control their own colors.

Local AI can summarize selected text or a bounded page excerpt and answer questions about that text. The distribution includes the CPU runtime. Choose Download AI model to fetch the separate Qwen3 0.6B model (about 523 MB); opening the panel does not start a download. Setup shows progress and can be cancelled. Keep Yas open while setup runs. Compatible models from an existing local Ollama installation are optional alternatives.

The bundled runtime starts only when needed, stores models under the selected profile's `ai` directory, and stops when Yas exits. Generation requests ask the model to unload afterward. Questions and answers remain in memory and are not added to browsing history or the profile database. There is no cloud fallback. Summaries can be inaccurate; the model cannot perform browser actions. AI performance and memory use depend on the model and the available RAM.

History suggestions omit temporary sign-in links containing account-routing or token parameters. Saved history remains available, and explicit bookmarks are preserved. This reduces accidental replay of old organization-specific sign-in links; it does not override a website's legitimate account redirects.

## Validation and performance

Version 0.4.9 migrates application records into `yas.redb`, with ordered binary records and Windows-encrypted vault/download payloads. The database cache budget is 8 MiB. Legacy files remain recovery copies; exports and diagnostics may still use JSON. The WebView2 profile is separate and keeps its existing engine-managed format. Copy the database and its `yas.storage-version` marker together when restoring a verified backup; do not replace an authoritative database with stale legacy files.

Current storage-migration, native regression, appearance/search, local-AI, package, and actual-update reports must name version 0.4.9 and its exact executable hash. Personal preservation proofs stay private. The new database does not itself establish faster website loading; only its scoped storage benchmarks describe measured operations. Local AI has no cloud fallback and uses a separate on-demand model download; runtime/model details and notices ship with the distribution.

### Previous 0.4.8 patch validation scope

Version 0.4.8 is a focused sleeping-tab title patch. `PATCH_RELEASE_TEST_RESULTS.json` and `TAB_TITLE_TEST_RESULTS.json`, when present with the final 0.4.8 executable hash, record its fresh validation. Regular session titles are saved with each URL; older URL-only sessions use matching local history/bookmarks or the website hostname. Stealth tabs remain excluded and restored pages still load only when selected. Older 0.4.7 evidence is historical and does not validate the patch binary.

### Previous 0.4.7 validation scope

Use only reports that identify version 0.4.7 and the published executable SHA-256 when assessing this release. A historical report beside newer source does not validate the newer executable. `QUALITY_CHECK_RESULTS.json` records completed release checks; `UPDATE_TEST_RESULTS.json` separately records the actual installation and profile-preservation result. Functional checks use isolated profiles and synthetic data. Native notification display and SmartScreen enforcement require their separately reported observations; API settings alone do not establish either result.

`VIEW_POLICY_TEST_RESULTS.json` distinguishes observed five-tab state retention from fallback or transition runs. Pressure thresholds and suspension-failure handling in `VIEW_POLICY_UNIT_TEST_RESULTS.json` are deterministic unit evidence, not a claim that physical memory pressure or native COM faults were induced. `RESOURCE_TEST_RESULTS.json` describes synthetic idle and residency checks.

If a completed `OPTIMIZATION_TEST_RESULTS.json` is supplied, it contains the matched 0.4.6-to-0.4.7 comparison, raw workload bindings, exclusions, and limits. Missing, draft, incomplete, or `scored:false` output is not a completed comparison. Earlier `PERFORMANCE_TEST_RESULTS.json` reports for 0.4.6 describe that historical Chrome/Edge comparison. Retaining more pages can use more RAM while avoiding reloads; report those costs alongside return time and CPU. No universal speed, memory, CPU, or smoothness winner is established.

Development microbenchmarks cover isolated download-ledger decisions, cosmetic class/ID extraction, and filter-engine initialization, including the cache's validation work when stated. They do not measure complete browser startup, public-site loading, total-browser RAM, or smoothness. Final results belong in version/hash-matched reports; pending 0.4.7 measurements are not implied by these implementation notes.

## Background downloads and reopening

Closing the window with X while live regular downloads need it hides the window and keeps the native engine running in the tray. Active transfers continue and paused transfers remain paused. Choose Open from the tray or open Yas from the Start menu to reveal the existing instance for that profile. Separate `--user-data-dir` profiles remain separate instances.

The private-background choice is off by default. Without it, closing the window ends Stealth browsing and cancels its unfinished transfers. Enabling it deliberately keeps the required private context alive while private jobs run; it does not save private download history. Explicitly closing the last Stealth tab still ends that private session.

Choose Exit from the tray when you want Yas to stop completely. This requests cancellation of unfinished native jobs and ends Stealth sessions. A crash, forced termination, sign-out, or Windows restart also ends the process. On the next launch, saved regular history may describe those jobs as Interrupted, but their old native operations no longer exist. Reopening the tray window is the same running process; restarting Yas after it exited is a new process.

## Data

Yas stores application records in `yas.redb`, its `yas.storage-version` marker, filter caches, models, and WebView2 profile data under `%LOCALAPPDATA%\Yas\Yas\data`. Vault and download-history encryption is bound to the current Windows account and its DPAPI keys; these records are not portable password/download-session sync. Legacy JSON and encrypted files remain unchanged recovery inputs after migration. Yas itself implements no telemetry service. The upgrade copies the complete WebView2 data directory after the previous browser closes; an internal legacy website profile identifier is retained when necessary so that existing cookies, local passwords, and extensions stay accessible. Fresh profiles use Yas identifiers. Websites, extensions, your selected search provider, filter-list hosts, model-download hosts, and WebView2 may make their own network connections.

## Build

Install Rust stable for `x86_64-pc-windows-msvc`, MSVC C++ Build Tools, and a Windows SDK. From this folder:

```powershell
.\build.ps1 -Test
.\build.ps1 -Run
.\build.ps1 -Release
.\build.ps1 -Debug
.\package.ps1 -Executable .\target\release\yas.exe
```

The default build is a full Cargo release build with optimized dependencies and application code, no debug assertions, and the Windows GUI subsystem. `-Debug` selects a normal development build. The script discovers MSVC and Windows SDK tools, compiles icon/version resources directly with the SDK, then links those resources into the application. This removes the app's unnecessary unsigned Rust build-helper executable that Windows Application Control blocked. Wry's developer-tools feature is explicitly enabled so the browser's developer-tools command compiles in release mode. No Windows security settings were changed.

Run `./quality.ps1 -TargetDirectory 'C:\path\to\qa-target'` for formatting verification, strict Clippy across all targets, and locked tests. It writes `QUALITY_GATE_RESULTS.json` with source/lock fingerprints and results. An optional dependency advisory audit requires explicit `-AuditExecutable 'C:\path\to\cargo-audit.exe' -AdvisoryDatabase 'C:\path\to\new-advisory-db'`; use a verified official tool and a new database directory. The gate fetches the database, records advisory categories and warnings, and does not install tools or launch Yas.

For packaging, install Inno Setup and provide the official x64 Microsoft MSVC app-local runtime DLLs in `runtime/`. `package.ps1` creates the installer and portable ZIP and validates the release version. On this computer, the signed compiler is in `work/installer-tools/Inno Setup 6/`. Use `-Compiler` to select another installation. Pass `-TargetDirectory` to `build.ps1` or set `CARGO_TARGET_DIR` to keep build intermediates outside this source folder.

`Yas.exe --user-data-dir "C:\path\to\profile"` selects a separate browser data directory, useful for isolated testing. On first launch, Yas copies your previous browser profile into its new data directory while preserving bookmarks, passwords, autofill, and regular website data. The previous browser must be closed. The original data stays available for rollback.

For a prepared browser-data JSON export, close Yas before using `Yas.exe --import-data "C:\path\to\export.json" --import-passwords "C:\path\to\passwords.csv" --import-report "C:\path\to\report.json"`. Password CSV is optional. Backups and reports contain counts rather than password values. The encrypted vault backup is bound to the current Windows account and is not a portable cross-computer password format. Plaintext password exports should be removed once you have verified the migration.

See `branding/BRAND.md` for the identity and `THIRD_PARTY.md` for filter attributions and source links.


Downloads use WebView2 deferrals to show the save dialog outside native callbacks. Cancel at the save prompt or use Downloads > Cancel during a transfer. Cancellation is shown separately from failure; closing a downloading tab also requests cancellation. Closing the window to the tray preserves eligible live jobs in the same process; explicit tray Exit stops them. Tiny hidden blank profile anchors preserve session cookies when page renderers are unloaded.

