# Truedesk for Android and iOS

The phone and tablet app: the browser viewer (`../web`) in a
[Tauri 2](https://v2.tauri.app) app window. Sessions run in the web view
(the `web-core` wasm build), so the Rust side only opens the window.

CI builds a signed Android APK on every change (see
`.github/workflows/android.yml`).

## Build it yourself

Needs Rust, Node.js, `wasm-bindgen-cli` 0.2.129, JDK 17 and the Android SDK
and NDK (`ANDROID_HOME` and `NDK_HOME` set).

```bash
cd web && npm ci && npm run wasm && cd ..          # the viewer's wasm build
cd mobile && npm ci
npx tauri android init                             # generates src-tauri/gen/android
npx tauri android dev                              # run on a connected phone or emulator
npx tauri android build --apk                      # release APK (sign it before installing)
```

Set `TRUEDESK_DEFAULT_RELAY` (a `wss://…/ws` URL) when building to choose the
relay the app starts with; it can be changed in the app.

iOS works the same way with `tauri ios init` / `tauri ios build` on a Mac,
but needs an Apple developer team to sign the app.
