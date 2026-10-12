// Builds the Tauri updater manifest (latest.json) from a release's signed
// bundles. Usage:
//   node updater-manifest.mjs <artifact-dir> <tag> <repo> <out> [previous.json]
// Exits 0 without writing <out> when nothing is signed or when previous.json
// already announces a newer version, so a re-run of an old tag can't roll
// installed apps back.
import { readdirSync, readFileSync, statSync, writeFileSync, existsSync } from "node:fs";
import { basename, join } from "node:path";

// Updater bundles by file suffix, and the manifest keys that point at them.
// The plugin looks up `{os}-{arch}-{installer}` first, then `{os}-{arch}`.
const TARGETS = [
  { suffix: ".app.tar.gz", keys: ["darwin-aarch64-app", "darwin-aarch64"] },
  { suffix: "-setup.exe", keys: ["windows-x86_64-nsis", "windows-x86_64"] },
  { suffix: ".msi", keys: ["windows-x86_64-msi"] },
  { suffix: ".AppImage", keys: ["linux-x86_64-appimage", "linux-x86_64"] },
  { suffix: ".deb", keys: ["linux-x86_64-deb"] },
  { suffix: ".rpm", keys: ["linux-x86_64-rpm"] },
];

const files = (dir) => readdirSync(dir).flatMap((name) => {
  const path = join(dir, name);
  return statSync(path).isDirectory() ? files(path) : [path];
});

// Semver precedence, enough for vX.Y.Z[-pre] tags.
export function compareVersions(a, b) {
  const parse = (v) => {
    const [core, pre] = v.split(/-(.*)/s);
    return { core: core.split(".").map(Number), pre: pre ? pre.split(".") : [] };
  };
  const x = parse(a), y = parse(b);
  for (let i = 0; i < 3; i++) if (x.core[i] !== y.core[i]) return x.core[i] - y.core[i];
  if (!x.pre.length || !y.pre.length) return y.pre.length - x.pre.length;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i], q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    const pn = /^\d+$/.test(p), qn = /^\d+$/.test(q);
    if (pn && qn && Number(p) !== Number(q)) return Number(p) - Number(q);
    if (pn !== qn) return pn ? -1 : 1;
    if (p !== q) return p < q ? -1 : 1;
  }
  return 0;
}

export function buildManifest(paths, tag, repo, now = new Date()) {
  const version = tag.replace(/^v/, "");
  const platforms = {};
  for (const { suffix, keys } of TARGETS) {
    const bundle = paths.find((path) => path.endsWith(suffix) && paths.includes(`${path}.sig`));
    if (!bundle) continue;
    const url = `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(basename(bundle))}`;
    const signature = readFileSync(`${bundle}.sig`, "utf8").trim();
    for (const key of keys) platforms[key] = { signature, url };
  }
  if (!Object.keys(platforms).length) return null;
  return {
    version,
    notes: `https://github.com/${repo}/releases/tag/${tag}`,
    pub_date: now.toISOString().replace(/\.\d{3}Z$/, "Z"),
    platforms,
  };
}

if (process.argv[1] && import.meta.url.endsWith(basename(process.argv[1]))) {
  const [dir, tag, repo, out, previous] = process.argv.slice(2);
  if (!dir || !tag || !repo || !out) {
    console.error("usage: updater-manifest.mjs <artifact-dir> <tag> <repo> <out> [previous.json]");
    process.exit(2);
  }
  const manifest = buildManifest(files(dir), tag, repo);
  if (!manifest) {
    console.log("No signed updater bundles found; leaving the manifest alone.");
    process.exit(0);
  }
  if (previous && existsSync(previous)) {
    const current = JSON.parse(readFileSync(previous, "utf8")).version;
    if (current && compareVersions(current, manifest.version) > 0) {
      console.log(`The manifest already announces ${current}; not replacing it with ${manifest.version}.`);
      process.exit(0);
    }
  }
  writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`Wrote ${out} for ${manifest.version}: ${Object.keys(manifest.platforms).join(", ")}`);
}
