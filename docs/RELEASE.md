# OpenDesk releases

Releases are unsigned Tauri bundles for macOS, Linux, and Windows.

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
4. When the builds finish, the workflow creates the GitHub release with
   `.github/RELEASE_NOTES.md` (install steps for unsigned builds) plus the
   generated changelog, and attaches the installers.

The workflow runs `npm run tauri build` from `viewer` and uploads only the
installers:

- macOS: `.dmg` (Apple silicon, macOS 12.3+)
- Linux: AppImage, `.deb`, and `.rpm`
- Windows: NSIS `.exe` and MSI

## Unsigned builds

There's no paid code signing or notarization on any platform. macOS builds
are ad-hoc signed (`signingIdentity: "-"` in `tauri.conf.json`), which turns
Gatekeeper's "damaged" error into a "can't verify the developer" prompt that
users can approve. Use these builds for testing, or distribute them to users who
accept the OS trust warnings. They carry no verified publisher identity.

- macOS may show Gatekeeper warnings. In Finder, use Control-click → Open, or approve the app in System Settings → Privacy & Security.
- Windows may show a SmartScreen warning. Use More info → Run anyway only if you trust the release source.
- Linux desktop environments may warn about an untrusted downloaded file. Mark the AppImage executable (`chmod +x OpenDesk*.AppImage`) and launch it, or install the `.deb` with the system package installer.
