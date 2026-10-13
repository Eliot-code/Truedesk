// Astro hashes the scripts it bundles, but some of Starlight's small inline
// scripts (search shortcut, theme picker, sidebar scroll) reach the page
// without a hash. After the build, add the hash of every inline script on a
// page to that page's own Content-Security-Policy meta tag, so the policy
// allows exactly the scripts that page ships and never 'unsafe-inline'.
import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const META = /<meta http-equiv="content-security-policy" content="([^"]*)">/;
const INLINE_SCRIPT = /<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g;

async function* htmlFiles(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* htmlFiles(path);
    else if (entry.name.endsWith(".html")) yield path;
  }
}

export function addInlineScriptHashes(html) {
  const meta = META.exec(html);
  if (!meta) return html;
  const directives = meta[1].split(";").map((part) => part.trim()).filter(Boolean);
  const index = directives.findIndex((part) => part.startsWith("script-src "));
  if (index === -1) throw new Error("page has a CSP without script-src");
  if (directives[index].includes("'unsafe-inline'")) throw new Error("script-src must not allow 'unsafe-inline'");
  const sources = new Set(directives[index].split(/\s+/).slice(1));
  for (const [, body] of html.matchAll(INLINE_SCRIPT)) {
    if (body.trim()) sources.add(`'sha256-${createHash("sha256").update(body).digest("base64")}'`);
  }
  directives[index] = `script-src ${[...sources].join(" ")}`;
  return html.replace(META, `<meta http-equiv="content-security-policy" content="${directives.join("; ")}">`);
}

export default function inlineScriptHashes() {
  return {
    name: "opendesk-inline-script-hashes",
    hooks: {
      "astro:build:done": async ({ dir, logger }) => {
        let pages = 0;
        for await (const file of htmlFiles(fileURLToPath(dir))) {
          const html = await readFile(file, "utf8");
          const updated = addInlineScriptHashes(html);
          if (updated !== html) {
            await writeFile(file, updated);
            pages += 1;
          }
        }
        logger.info(`checked inline script hashes on ${pages} page(s)`);
      },
    },
  };
}
