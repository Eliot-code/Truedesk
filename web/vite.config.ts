import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";

// Baked in at build time, like the desktop app. Source builds talk to a
// local development relay.
const defaultRelay = process.env.TRUEDESK_DEFAULT_RELAY || "ws://127.0.0.1:8080/ws";

// Only in production builds, so `vite dev` can use its own inline scripts
// and a local ws:// relay. Scripts and wasm come from this origin only; the
// one place data goes is a wss:// relay.
const csp = [
  "default-src 'none'",
  "script-src 'self' 'wasm-unsafe-eval'",
  "style-src 'self'",
  "img-src 'self'",
  "manifest-src 'self'",
  "connect-src 'self' wss:",
  "base-uri 'none'",
  "form-action 'none'",
].join("; ");

function contentSecurityPolicy(): Plugin {
  return {
    name: "truedesk-csp",
    apply: "build",
    transformIndexHtml: () => [{ tag: "meta", attrs: { "http-equiv": "Content-Security-Policy", content: csp }, injectTo: "head-prepend" }],
  };
}

export default defineConfig({
  // Relative URLs, so the app works under a GitHub Pages project path.
  base: "./",
  plugins: [react(), contentSecurityPolicy()],
  define: { __DEFAULT_RELAY__: JSON.stringify(defaultRelay) },
  // The video and input code is shared with the desktop viewer.
  server: { fs: { allow: [".."] } },
  // Never inline assets as data: URLs; the CSP only allows this origin.
  build: { assetsInlineLimit: 0 },
});
