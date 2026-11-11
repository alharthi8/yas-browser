# Yas 0.4.9

Application settings, ordered bookmarks and history, saved tab titles, preferences, the encrypted password/autofill vault, and encrypted download records move to the Rust redb database. First migration validates every legacy input, commits a staged database with immediate durability, reopens it and checks full data equality before publishing it. The original settings and encrypted files stay unchanged as recovery inputs. A storage-version marker prevents a missing migrated database from silently reimporting stale data. Existing invalid or unsupported databases stop startup rather than loading empty defaults. Passwords and download details remain Windows DPAPI protected; website-engine cookies and profiles keep their existing format and location.

Appearance preferences offer Light, Dark and System. Searches from the address bar and new-tab page use the selected Bing, Google, DuckDuckGo or Brave Search provider. These preferences are saved in the database.

Local AI provides manual summaries and questions about selected text or a limited page excerpt. The compact distribution includes a local runtime; a separate model download is initiated explicitly through setup. Responses are text, have no browser-action tools, and prompts/answers are not stored in the profile database. The model is loaded on demand and requested to unload after each completed generation. Existing compatible local models can also be used. Download size, setup and engine limits are described by the interface and matching release reports.

Address-bar history suggestions omit temporary sign-in URLs containing account-routing or token parameters. Existing history and explicitly saved bookmarks are retained. The Outlook shortcut continues to use the public Outlook entry; Yas does not select an organization or bypass website sign-in redirects. A clean Stealth diagnostic of the installed previous release reached the standard Microsoft sign-in page; this does not establish successful account authentication.

Only fresh 0.4.9 reports naming its exact executable hash validate this release. Migration, local-AI, preference, compatibility, package and actual-user update checks are separate. Storage API benchmarks do not establish website loading or whole-browser speed gains. Earlier release reports remain historical.

# Previous release: Yas 0.4.8

Sleeping and restored regular tabs keep their last saved page title. Existing URL-only sessions recover names from matching local history or bookmarks, with the website hostname as a fallback. Hovering a tab shows its full title, sleeping status, and address. Pages still load lazily. Stealth tab titles and URLs are excluded from saved sessions. The legacy URL array remains compatible with earlier releases.

This is a focused title-restoration patch. Only fresh reports naming 0.4.8 and its executable hash validate this build. Earlier 0.4.7 reports remain historical; this patch does not establish a new browser speed, RAM, or CPU comparison.

# Previous release: Yas 0.4.7

The normal recent-view budget increases to five ordinary website views. Memory pressure, an unavailable memory reading, or repeated failure to confirm native suspension falls back to two. Unprotected background views still expire after 120 seconds. Playing-audio pages and active or paused download owners are exempt from automatic trimming, expiry, and suspension. Retained views keep live document state; evicted or explicitly sleeping tabs reload and can lose unsaved state. This policy can spend more RAM to avoid reloads, rather than guaranteeing the lowest memory use.

Request filtering reuses a view's top-level host until its document context changes. It continues to apply current rules and site exceptions. An embedded compiled filter engine targets initialization of the exact unchanged bundled snapshots, with metadata/input validation and the normal text fallback for changed, updated, custom, or invalid inputs. Source inclusion does not establish that a particular release used the cache; its matching runtime report must show that branch.

Completion and measurements are recorded only in reports naming version 0.4.7 and the final executable hash. `VIEW_POLICY_TEST_RESULTS.json` must explicitly identify positive five-tab state retention; a fallback run alone does not prove it. Named policy units describe simulated thresholds and failure handling separately from native observations. `OPTIMIZATION_TEST_RESULTS.json`, when completed and scored, records the paired before/after workloads and limits. Pending or historical evidence does not validate this candidate. Isolated initialization microbenchmarks do not establish complete browser startup or a generally faster browser.

# Previous release: Yas 0.4.6

Website notifications now have an Allow/Block prompt and permission controls in Settings. Regular choices are saved locally; Stealth choices stay separate in memory and end with the last Stealth tab. Notifications require a loaded website page. Sleeping or closing its tab, or exiting Yas, stops that page's notifications. Service-worker Web Push is not implemented. Windows notification settings and Do Not Disturb can prevent banners.

Yas now explicitly requests Microsoft Defender SmartScreen website and download reputation checks and confirms the WebView2 setting. Windows security settings can override this request. A successful API readback does not certify actual enforcement; warning-page observations must be reported separately. Yas does not change Windows security settings.

Release-specific reports record automated browsing, autofill, downloads, notification permissions and privacy reset, session restoration, and isolated installer checks for the final executable. The native notification banner was observed; manual banner clicks and dismissal were not verified. A harmless Microsoft phishing demonstration displayed the native SmartScreen unsafe-site warning; this observation does not establish enforcement for every site or download. If PERFORMANCE_TEST_RESULTS.json is present, it records the final Chrome/Edge comparison, matched workloads, exclusions, and limits. Its absence means no completed comparative result is bundled. Measurements apply to this computer and test window and do not establish a universal browser winner. Development microbenchmarks cover isolated download-ledger preparation and cosmetic-selector scans, rather than total-browser performance. Earlier 0.4.5 evidence remains historical.

# Previous release: Yas 0.4.5

This maintenance release protects existing browsing data when saved settings are damaged or unreadable. Startup reports the problem and preserves the file so a verified backup can be restored.

Session restore keeps every saved regular tab, including sessions with more than 30 tabs, while loading pages lazily.

Downloads handle rapid Pause, Continue, and Cancel actions more reliably, clean up a transfer that could not be registered, and apply size limits before decrypting saved download history. Reopening the background browser can show the Downloads panel directly.

Address-bar typing now respects input-method composition. Ctrl+Shift+Tab moves backward through tabs, and Downloads controls and filters retain useful keyboard focus after an action.

Setup and uninstall now defer when Yas is running or its application file is in use. Current Setup also rejects replacement of a newer installed version; equal-version repairs remain available.

# Previous release: Yas 0.4.4

Closing the window with X can now keep live regular downloads running in the tray. Active jobs continue and paused jobs stay paused in the same process. Open from the tray or start Yas again for the same profile to bring that instance back instead of creating another profile writer. A separate `--user-data-dir` remains a separate instance. Explicit tray Exit ends the process and stops unfinished native jobs.

Regular download history now persists as Windows-account-encrypted metadata in `downloads.dpapi`. Completed entries can still refer to their saved files after relaunch. Unfinished entries from a previous process are restored honestly as Interrupted, without claiming that the lost native operation can Continue. True process exit, a crash, sign-out, and Windows restart do not preserve native resumability. Start a new download from the website when required. Downloaded file contents themselves are not encrypted by this metadata store.

Private background retention is an explicit choice and defaults to off. Without it, window close ends Stealth browsing and cancels its unfinished transfers. If enabled, private transfer jobs keep the necessary Stealth session and cookies alive until those jobs finish or the user ends the session. Closing the last Stealth tab still ends its private context. Private download records are excluded from persisted history in every mode; saved files and bookmarks remain as before.

Before updating or uninstalling, wait for live jobs or explicitly stop Yas through its tray Exit action. X may only hide the window. Replacing or terminating a running background process stops its jobs; encrypted history is not a cross-process download-resume implementation. The release-specific tests cover lifecycle signaling, native transfer behavior, persistence, and privacy; see the resulting JSON reports for the flows actually exercised.

# Previous release: Yas 0.4.3

The address bar now follows the actual top-level website after links, redirects, back/forward, and same-document URL changes. Keeping the bar focused no longer freezes the old address. Only an unsubmitted typing draft is preserved; submission, Escape, leaving the bar, or switching tabs restores the current website URL.

Downloads now has a modern card layout with All, Downloading, Paused, Completed, and Other filters. Cards show the filename/type, source host, destination, byte/percentage progress, active speed, and estimated remaining time. Unknown-size transfers use an indeterminate bar. Controls support native Pause, Continue when the server/runtime permits it, and Cancel. Completed files can be opened or their folder shown by an explicit click.

Paused and resumable interrupted transfers retain their native operation and source tab. Continue keeps the original download entry and destination, including when the native engine hands the resumed transfer to a new operation. Speed/ETA reset correctly across pauses or server restarts. Cancel and completion release their tab protection; normal app exit cancels unfinished operations. The browser refreshes resumability without spinning a new polling thread. These controls work during the current browser session; paused transfers are not restored after Yas closes.

This update includes the saved-site address-bar lookup, searchable password metadata, username-first autofill, honest fill-result messages, and hidden-field protections from the preceding fixes. Imported password/history data is retained. Test reports record exact hashes and the flows exercised; they do not certify every server or website.

# Included lookup and autofill fixes

# Yas 0.4.2

Typing a saved website name in the address bar now returns local suggestions from all imported history, bookmarks, and saved login origins. Previously, the address bar offered only navigation/web search, so migrated Zoho sites did not appear while typing. Results are limited to eight and queried locally; clicking a result or using Arrow keys then Enter opens it. Plain Enter still searches. Passwords and usernames never appear in the address-bar response. Stealth suggestions exclude regular history while retaining explicitly saved sites/bookmarks.

Passwords & autofill now has search by site, label, or username, result counts, stable row actions, and Open site. Filtering clears any revealed password immediately. Credential filling supports username-first forms and the later password-only step while retaining exact HTTPS-origin matching. Passwords are not inserted into hidden, disabled, or read-only fields. The app reports no matching field when nothing was filled. Address and generic form-value messages also report actual outcomes.

Release validation uses isolated profiles and synthetic credentials for browsing/autofill/download/Stealth tests. A separate local counts-only audit confirmed the user's encrypted migration without exposing credential values; personal audit files are excluded from distribution bundles. No actual account sign-in is asserted by the synthetic form checks. See the current JSON reports for exact build hashes and tested flows.

# Previous rename release

Yas 0.4.1

Yas is the new name throughout the browser interface, Windows executable and metadata, installer, shortcuts, exports, and source packages. The new original UAE-inspired falcon icon keeps the blue palette, and the interface continues to use Dubai. The new tagline is “A clearer view of the web.”

The first default-profile launch copies the previous profile into the Yas data directory only when no Yas profile already exists. The copy is staged and verified; the previous profile remains available for rollback. Migrated website cookies, local passwords, extensions, the encrypted vault, bookmarks, and history are preserved. Existing Yas data is never automatically overwritten. See RENAME_NOTES.md for compatibility details.

The installer retains its upgrade identity and retires an identified previous application using its own uninstaller before installing Yas. Browser data is separate and is preserved on uninstall. The installer and executable remain unsigned, as previously requested. The Microsoft-signed WebView2 helper and MSVC runtime resources remain included; the helper needs internet to install a missing WebView2 runtime and setup asks first.

The feature set carries forward full-page PNG and PDF export, Stealth mode, imports and explicit autofill, extensions, request/cosmetic blocking, background suspension, lazy tab restoration, and coalesced settings writes. PNG export covers the currently rendered document with a 32,767-pixel-side/128-megapixel limit; PDF uses the site’s print layout. Load lazy content first.

Microsoft account password sync, automatic migration of payment cards or Tabbit cookies/extensions, Chrome Web Store integration, extension toolbar popups/native messaging, and universal site/extension compatibility are not implemented. Performance superiority over other browser products has not been established. Prior local-fixture measurements do not establish a comparative result for this rename.

Release-specific validation is recorded in the JSON reports beside this file. Tests use isolated profiles and synthetic credentials.

### In-app update channel

Added an opt-out daily check, manual Check for updates, verified installer download/cancellation, and restart-to-update with transfer/AI deferral and a private storage backup. The signed GitHub feed uses the existing public verification key. The complete per-user installer updates bundled runtime files as well as the browser. Portable copies require manual installation. The account sign-in feature was canceled; guided local import remains available. Release-specific tests and publication are required before claiming an over-the-air release is live.
