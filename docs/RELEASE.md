# OpenDesk releases

OpenDesk releases are built as unsigned Tauri bundles for macOS, Linux, and Windows.

## Cut a release

1. Push a version tag matching `v*`, for example `git tag v0.1.0 && git push origin v0.1.0`.
2. The `Release unsigned bundles` workflow builds all three platforms in parallel.
3. After the builds finish, the workflow creates the GitHub release and attaches the downloaded bundle artifacts.

The workflow runs `npm run tauri build` from `viewer`. The configured targets are:

- macOS: `.app` and `.dmg`
- Linux: AppImage and `.deb`
- Windows: NSIS `.exe` and MSI

## Unsigned build caveat

No code signing, notarization, certificates, or Windows signing are configured. These artifacts are suitable for testing and distribution where users accept the trust warnings, but they do not provide platform publisher verification.

- macOS may show Gatekeeper warnings. In Finder, use Control-click → Open, or approve the app in System Settings → Privacy & Security.
- Windows may show a SmartScreen warning. Use More info → Run anyway only if you trust the release source.
- Linux desktop environments may warn about an untrusted downloaded file. Mark the AppImage executable (`chmod +x OpenDesk*.AppImage`) and launch it, or install the `.deb` with the system package installer.
