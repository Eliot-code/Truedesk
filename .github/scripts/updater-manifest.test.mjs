// Run with: node --test .github/scripts/updater-manifest.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildManifest, compareVersions } from "./updater-manifest.mjs";

test("orders versions by semver precedence", () => {
  const sorted = ["0.1.0", "0.1.0-beta.10", "0.1.0-beta.2", "0.0.9", "0.1.0-alpha", "0.2.0-beta.1"]
    .sort(compareVersions);
  assert.deepEqual(sorted, ["0.0.9", "0.1.0-alpha", "0.1.0-beta.2", "0.1.0-beta.10", "0.1.0", "0.2.0-beta.1"]);
});

test("lists only bundles that have a signature", () => {
  const dir = mkdtempSync(join(tmpdir(), "manifest-"));
  const path = (name) => join(dir, name);
  const names = [
    "OpenDesk_0.1.0-beta.5_aarch64.app.tar.gz", "OpenDesk_0.1.0-beta.5_aarch64.app.tar.gz.sig",
    "OpenDesk_0.1.0-beta.5_x64-setup.exe", "OpenDesk_0.1.0-beta.5_x64-setup.exe.sig",
    "OpenDesk_0.1.0-beta.5_amd64.AppImage", "OpenDesk_0.1.0-beta.5_amd64.AppImage.sig",
    "OpenDesk_0.1.0-beta.5_aarch64.dmg", "OpenDesk_0.1.0-beta.5_x64_en-US.msi",
  ];
  for (const name of names) writeFileSync(path(name), name.endsWith(".sig") ? `sig of ${name}\n` : "");

  const manifest = buildManifest(names.map(path), "v0.1.0-beta.5", "treeot/opendesk", new Date("2026-10-11T12:00:00.123Z"));
  assert.equal(manifest.version, "0.1.0-beta.5");
  assert.equal(manifest.pub_date, "2026-10-11T12:00:00Z");
  assert.deepEqual(Object.keys(manifest.platforms).sort(), [
    "darwin-aarch64", "darwin-aarch64-app", "linux-x86_64", "linux-x86_64-appimage", "windows-x86_64", "windows-x86_64-nsis",
  ]);
  assert.deepEqual(manifest.platforms["windows-x86_64"], {
    signature: "sig of OpenDesk_0.1.0-beta.5_x64-setup.exe.sig",
    url: "https://github.com/treeot/opendesk/releases/download/v0.1.0-beta.5/OpenDesk_0.1.0-beta.5_x64-setup.exe",
  });
});

test("returns null when nothing is signed", () => {
  assert.equal(buildManifest(["/x/OpenDesk.dmg"], "v1.0.0", "treeot/opendesk"), null);
});
