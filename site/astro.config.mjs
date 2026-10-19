// @ts-check
import { defineConfig } from "astro/config";
import process from "node:process";
import react from "@astrojs/react";
import starlight from "@astrojs/starlight";
import inlineScriptHashes from "./src/integrations/inline-script-hashes.mjs";

// Baked into the browser viewer at build time, like the desktop app. Local
// builds talk to a local development relay.
const defaultRelay = process.env.TRUEDESK_DEFAULT_RELAY || "ws://127.0.0.1:8080/ws";

const repo = "https://github.com/Eliot-code/truedesk";

export default defineConfig({
  // Set `site` to the URL this is served from to get canonical links and a
  // sitemap. Pages are served from the domain root, so it needs a custom
  // domain rather than a /truedesk/ project-page path.
  trailingSlash: "always",
  // Starlight highlights code with Expressive Code; Astro's own Shiki pass
  // isn't used.
  markdown: { syntaxHighlight: false },
  integrations: [
    react(),
    starlight({
      title: "Truedesk",
      description: "Self-hosted, end-to-end encrypted remote desktop.",
      logo: { src: "./src/assets/logo.svg", alt: "" },
      favicon: "/favicon.svg",
      social: [{ icon: "github", label: "GitHub", href: repo }],
      editLink: { baseUrl: `${repo}/edit/main/site/` },
      customCss: ["./src/styles/docs.css"],
      routeMiddleware: "./src/docs-csp.ts",
      // src/pages/404.astro covers the whole site, not just the docs.
      disable404Route: true,
      sidebar: [
        { label: "Overview", link: "/docs/" },
        "docs/getting-started",
        "docs/sessions",
        "docs/host-controls",
        "docs/tunnels",
        "docs/wake-on-lan",
        "docs/unattended",
        "docs/browser-viewer",
        "docs/mobile",
        "docs/virtual-displays",
        "docs/self-hosting",
        "docs/security",
        "docs/updates",
        "docs/troubleshooting",
        {
          label: "Elsewhere",
          items: [
            { label: "Home", link: "/" },
            { label: "Browser viewer", link: "/connect/" },
            { label: "Releases", link: `${repo}/releases` },
          ],
        },
      ],
    }),
    inlineScriptHashes(),
  ],
  security: {
    // A hash-based policy in a <meta> tag on every page: scripts and styles
    // come from this origin or are inline ones Astro hashed at build time.
    // Pages add what they need on top (see src/docs-csp.ts and
    // src/pages/connect.astro).
    csp: {
      directives: [
        "default-src 'none'",
        "img-src 'self'",
        "font-src 'self'",
        "connect-src 'self'",
        "base-uri 'none'",
        "form-action 'none'",
      ],
      scriptDirective: { resources: ["'self'"] },
      styleDirective: { resources: ["'self'"] },
    },
  },
  vite: {
    define: { __DEFAULT_RELAY__: JSON.stringify(defaultRelay) },
    // The viewer's source lives in ../web and ../viewer, which may have their
    // own node_modules; keep one copy of React.
    resolve: { dedupe: ["react", "react-dom"] },
    server: { fs: { allow: [".."] } },
    // Never inline assets as data: URLs; the policy only allows this origin.
    build: { assetsInlineLimit: 0 },
  },
});
