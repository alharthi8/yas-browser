# Yas 0.4.7 — naming and upgrade compatibility

The Yas identity introduced in 0.4.1 remains unchanged. An existing 0.4.6 Yas installation upgrades under the same application identity; its profile stays outside the installation folder. Explicitly exit the background browser before updating. Preserving download history does not preserve live native transfers across process exit.

The browser name is Yas throughout its interface, window title, Windows file metadata, executable, shortcuts, installer, portable package, export filenames, source package, and branding. The original UAE-inspired falcon symbol replaces the previous icon. The interface retains Dubai and the modern blue light/Stealth themes.

Yas stores its default profile at `%LOCALAPPDATA%\Yas\Yas\data`. On first launch, it copies the previous profile into a temporary sibling directory, verifies the settings and encrypted vault, and commits the copy only when complete. The previous browser must be closed. Existing Yas data is never overwritten or automatically merged.

The old profile remains as a rollback copy. Migrated WebView2 website data retains its existing internal profile identifier; changing that opaque identifier would open an empty cookie/password/extension profile. New installations use Yas identifiers. Old new-tab URLs are accepted only as an upgrade compatibility alias and become `yas://newtab`.

The installer keeps its stable upgrade identity, retires an identified previous installation using that installation's own uninstaller, and installs Yas with new shortcuts. Browser data is outside the application installation and is preserved on upgrade and uninstall.
