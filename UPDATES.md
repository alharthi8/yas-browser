# Signed GitHub updates

Windows previews check GitHub on startup and from Settings > Check for app updates. Startup checks fail quietly offline; manual checks report failures. Users approve an update. Its signed ZIP is checked against the public key in release.pub, then the running executable is replaced using self_update/self-replace. Restart when convenient; the app does not kill active sends, downloads, or sessions. Fully quit Yas Browser through its tray when downloads keep it running.

Install the 0.4.6 bootstrap installer once. Older previews do not contain the updater. User data is outside the executable and retained. This channel updates the executable; changes requiring new runtimes or installation layout need a new full installer. The updater applies per-user writable installations and portable copies; it does not request administrator rights.

Maintainers: update Cargo version and branding/yas.rc, commit, then push a matching vVERSION tag. The Windows workflow builds/tests, creates a version-bound signed ZIP, verifies it using the built app, rejects a corrupted package, creates a draft with the bootstrap installer, then publishes. GitHub signing secret: YAS_UPDATE_SIGNING_KEY. Private keys never belong in source or release assets. Back up and retain the existing private key; replacing the key breaks existing clients unless key rotation support ships first.

The artifact signature is separate from Windows Authenticode publisher signing. macOS updates are not enabled until a macOS port, signing/notarization, and tested update flow exist.
