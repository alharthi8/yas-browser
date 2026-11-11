# Yas Browser 1.0.0

Designed and built in the UAE by Appana Technologies. A Rust/Tao/Wry desktop browser using Microsoft's WebView2 engine. Yas-owned code is MIT licensed; third-party code and runtime components retain their own licenses.

The Windows 1.0 launch is being validated on the codex/browser-v1 branch. Published downloads appear on [yasapps.com](https://yasapps.com/#downloads) only after the release checks pass. Yas Mail and other products remain coming soon. The separate macos/ port has fewer features and is not Apple notarized; its release status is independent of Windows.

## Install

Windows 10 22H2 or Windows 11, x64. Use the full 1.0.0 installer once, including when upgrading from an older development preview. The package includes the reviewed CPU-only local AI runtime and public automation helper, but no model, user browsing profile, passwords, mail, or private test data. The optional model download starts only through explicit AI setup. AI generation can use substantial CPU and RAM. Windows publisher signing is not configured; OS policy may warn or block the app.

Read [RELEASE_v1.md](RELEASE_v1.md) for features, install/update guidance, and important limitations. [UPDATES.md](UPDATES.md) documents the signed GitHub update channel. Historical development details and results are in [WINDOWS_DEVELOPMENT_NOTES.md](WINDOWS_DEVELOPMENT_NOTES.md); they do not establish validation of a newly built executable.

## Build

Install Rust stable/MSVC, Visual Studio C++ Build Tools, a Windows SDK, Python 3.12+, and WebView2. Node 24 runs the optional native automation test. The reviewed CPU runtime is reconstructed from a pinned upstream archive; every packaged file is checked against its reviewed hash. No model is downloaded by this build command.

~~~powershell
python scripts/prepare-ai-runtime.py
./build.ps1 -GenerateBundledCache -Release
./build.ps1 -Test
./build.ps1 -Release
node scripts/native-smoke.mjs target/release/yas.exe
~~~

Compiled filter-cache metadata is bound to the exact Cargo/source inputs and build environment. Regenerate it after dependency or bound-source changes. Unverified caches fall back to parsing the text lists.

## Releases

Update the Cargo, resource and installer versions, regenerate the compiled filter cache, update release notes, run checks, and push a matching vVERSION tag. The Windows workflow rebuilds and tests, checks an actual native WebView2 launch/interaction, verifies the runtime payload, signs the update ZIP using the existing GitHub signing secret, checks signature acceptance/tamper rejection, and publishes installers with checksums. Never commit private signing keys or user profiles.

Account sync and paid Google/Microsoft account integrations are not included. The local import/vault features do not require a paid cloud subscription. No universal RAM, CPU, battery or browser-speed advantage is claimed.
