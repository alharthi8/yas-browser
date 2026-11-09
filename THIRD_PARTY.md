# Third-party components

- **Wry** 0.57.1 and **Tao** 0.37.2: Tauri project, MIT/Apache-2.0. https://github.com/tauri-apps/wry and https://github.com/tauri-apps/tao
- **adblock-rust** 0.13.5: Brave authors, Mozilla Public License 2.0. https://github.com/brave/adblock-rust . Yas uses the crate without source modifications. Exact source is available from https://crates.io/api/v1/crates/adblock/0.13.5/download . The MPL license text is bundled in `licenses/MPL-2.0.txt`.
- **EasyList** and **EasyPrivacy**: The EasyList authors (https://easylist.to/). Unmodified snapshots dated 8 November 2026 are in `filters/`, preserving source headers. Distributed under Creative Commons Attribution-ShareAlike 3.0 Unported (one of the authors' dual-license options). License: https://creativecommons.org/licenses/by-sa/3.0/ ; policy: https://easylist.to/pages/licence.html . Source: https://github.com/easylist/easylist . Full CC license text is in `licenses/CC-BY-SA-3.0.txt`.
- **Dubai** typeface: used from the operating system; not redistributed.
- **Microsoft Edge WebView2**: the full runtime is installed separately. A Microsoft-signed Evergreen bootstrapper is included under `prerequisites/` in portable/installed packages; provenance and SHA256 are recorded there. It requires internet to download the runtime, and setup asks before running it. https://developer.microsoft.com/microsoft-edge/webview2/
- **Microsoft Visual C++ Runtime**: unmodified, Microsoft-signed x64 `vcruntime140.dll` and `vcruntime140_1.dll` app-local redistributable binaries from Visual Studio's Redist directory. Included adjacent to the application in installer/portable packages, retaining Microsoft's embedded copyright and signature. https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files

Other Rust dependencies and exact versions are recorded in `Cargo.lock`. Their license files accompany the source distribution in `licenses/dependencies/`.
