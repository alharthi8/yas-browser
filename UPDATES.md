# Yas Browser updates

The installed Windows browser checks once daily while regular browsing is active.
Turn this off under Settings → Yas Browser updates, or check manually there.
Checks request one fixed public GitHub release file; they do not send browsing
history, passwords, page content, or account identities. GitHub receives the
ordinary network request, including its source IP address.

An available update is downloaded only after the user chooses Download update.
Yas verifies the signed release metadata with the existing `release.pub` key,
then checks the complete installer's exact size and SHA-256 digest. Redirects
are limited to this repository's GitHub release paths and GitHub's release asset
host. Invalid, expired, altered, oversized, or downgraded releases fail closed.

Restart to update requires all browser transfers, paused transfers, and AI tasks
to finish or be canceled first. It saves regular tabs, ends Stealth, and waits for
the exact browser process to exit. A private, byte-verified backup of application
storage files is made while holding the profile lock. The full per-user installer
updates the browser and its bundled runtime; it never forcibly closes another
browser process. The updated executable version and profile are checked before
Yas reopens. If installation or verification fails, the private backup remains.

Backups live in `update-backups` under the browser's profile directory. They can
contain encrypted passwords and browsing records; do not publish them. Website
engine profiles, model weights, and downloaded user files remain outside the
application installation and are not overwritten by Setup.

Portable copies require a manual full-installer update. Earlier previews without
this full-installer updater need the new bootstrap installer once. Windows may
still apply its normal policy to unsigned executables: release signatures are
separate from Windows Authenticode publisher signing.

## Maintainer release process

Reuse the existing `alharthi8/yas-browser` repository and its protected
`YAS_UPDATE_SIGNING_KEY` secret. Never replace the pinned key or commit a private
key. Existing public-key identity is retained; private signing material is used
only in memory in the reviewed release job. No paid updater service is required.

1. Bump the Cargo, Windows resource, installer, and interface versions together.
2. Build and run the release quality, native browser, migration, update, and
   package tests. Keep exact executable and installer hashes in their reports.
3. Run `node tools/prepare-update-release.mjs --output FRESH_DIRECTORY` after
   finalization. Upload only its full installer and public `prevalidated.json`
   to a draft `vVERSION` release, with the reviewed source on `main`.
4. Run the manual Reviewed Windows update release workflow on `main`. It repeats
   source and installer verification, signs metadata in a separate job, and
   checks that the installed browser accepts the signature and rejects tampering.
   Its default keeps the release as a draft. Set `publish` only when ready to
   release the reviewed files to everyone. Keep macOS prereleases separate.
5. Check the public signed feed and download again after publication. The signed
   feed expires after 90 days; refresh its signature before expiration if no new
   version is released. Expired feeds cannot authorize an installation.

Signing and uploading a draft are not publication. Publishing a release makes it
available to all clients. The browser never silently installs it or interrupts
the user's active work.
