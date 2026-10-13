import { defineRouteMiddleware } from "@astrojs/starlight/route-data";

// Additions to the site-wide policy (astro.config.mjs) for docs pages only.
export const onRequest = defineRouteMiddleware((context) => {
  // Search (Pagefind) looks up its index in WebAssembly.
  context.csp.insertScriptResource("'wasm-unsafe-eval'");
  // Code block and checklist icons are small SVGs in data: URLs.
  context.csp.insertDirective("img-src 'self' data:");
  // Code block colours and sidebar nesting are set in style="" attributes.
  // This allows attributes only, not <style> elements or any script. (The
  // build warns that style-src's 'self' doesn't carry over; that's fine.)
  context.csp.insertStyleResource({ resource: "'unsafe-inline'", kind: "attribute" });
});
