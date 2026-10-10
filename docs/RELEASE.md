# OpenDesk releases

OpenDesk releases are built as unsigned Tauri bundles for macOS, Linux, and Windows.

## Cut a release

1. Once: set the repo variable `OPENDESK_DEFAULT_RELAY` (Settings → Secrets
   and variables → Actions → Variables) to the public relay, e.g.
   `wss://relay.example.com/ws`. It becomes the app's default relay; the
   release fails without it.
2. Push a version tag matching `v*`, for example `git tag v0.1.0 && git push origin v0.1.0`.
   A tag with a suffix (`v0.1.0-beta.1`) is published as a pre-release. Keep
   the version in `tauri.conf.json` numeric (`0.1.0`): Windows MSI rejects
   pre-release labels.
3. The `Release unsigned bundles` workflow builds all three platforms in parallel.
4. After the builds finish, the workflow creates the GitHub release with
   `.github/RELEASE_NOTES.md` (install steps for unsigned builds) plus the
   generated changelog, and attaches the bundles.

The workflow runs `npm run tauri build` from `viewer`. The configured targets are:

- macOS: `.app` and `.dmg`
- Linux: AppImage and `.deb`
- Windows: NSIS `.exe` and MSI

## Unsigned build caveat

No code signing, notarization, certificates, or Windows signing are configured. These artifacts are suitable for testing and distribution where users accept the trust warnings, but they do not provide platform publisher verification.

- macOS may show Gatekeeper warnings. In Finder, use Control-click → Open, or approve the app in System Settings → Privacy & Security.
- Windows may show a SmartScreen warning. Use More info → Run anyway only if you trust the release source.
- Linux desktop environments may warn about an untrusted downloaded file. Mark the AppImage executable (`chmod +x OpenDesk*.AppImage`) and launch it, or install the `.deb` with the system package installer.
