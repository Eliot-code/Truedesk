// Download links for the newest release, looked up once at build time. Every
// beta is a pre-release, which GitHub's /releases/latest skips, so this reads
// the release list. Without a token (local builds, CI checks) or if the API
// fails, the page links to the releases page instead.
import process from "node:process";

export const REPO = "treeot/opendesk";
export const RELEASES_URL = `https://github.com/${REPO}/releases`;

export interface Download {
  label: string;
  detail: string;
  url: string;
}

export interface Release {
  tag: string;
  url: string;
  prerelease: boolean;
  mac: Download[];
  windows: Download[];
  linux: Download[];
}

interface ApiAsset { name: string; browser_download_url: string }
interface ApiRelease { tag_name: string; html_url: string; draft: boolean; prerelease: boolean; published_at: string | null; assets: ApiAsset[] }

const KINDS: { os: "mac" | "windows" | "linux"; pattern: RegExp; label: string; detail: string }[] = [
  { os: "mac", pattern: /_aarch64\.dmg$/, label: "Download for Mac", detail: ".dmg · Apple silicon · macOS 12.3+" },
  { os: "windows", pattern: /-setup\.exe$/, label: "Download installer", detail: ".exe · 64-bit · Windows 10+" },
  { os: "windows", pattern: /\.msi$/, label: "MSI package", detail: ".msi · for managed installs" },
  { os: "linux", pattern: /\.AppImage$/, label: "AppImage", detail: "x86_64 · updates itself" },
  { os: "linux", pattern: /\.deb$/, label: ".deb", detail: "Debian, Ubuntu" },
  { os: "linux", pattern: /\.rpm$/, label: ".rpm", detail: "Fedora, openSUSE" },
];

export function pickRelease(releases: ApiRelease[]): Release | null {
  const newest = releases
    .filter((release) => !release.draft && release.tag_name.startsWith("v") && release.assets.length > 0)
    .sort((a, b) => (b.published_at ?? "").localeCompare(a.published_at ?? ""))[0];
  if (!newest) return null;
  const release: Release = { tag: newest.tag_name, url: newest.html_url, prerelease: newest.prerelease, mac: [], windows: [], linux: [] };
  for (const kind of KINDS) {
    const asset = newest.assets.find((item) => kind.pattern.test(item.name));
    if (asset) release[kind.os].push({ label: kind.label, detail: kind.detail, url: asset.browser_download_url });
  }
  return release;
}

let cached: Promise<Release | null> | undefined;

export function latestRelease(): Promise<Release | null> {
  cached ??= fetchLatest();
  return cached;
}

async function fetchLatest(): Promise<Release | null> {
  const token = process.env.GITHUB_TOKEN;
  if (!token) return null;
  try {
    const response = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=20`, {
      headers: { Accept: "application/vnd.github+json", Authorization: `Bearer ${token}`, "X-GitHub-Api-Version": "2022-11-28" },
      signal: AbortSignal.timeout(15_000),
    });
    if (!response.ok) throw new Error(`GitHub API returned ${response.status}`);
    return pickRelease((await response.json()) as ApiRelease[]);
  } catch (error) {
    console.warn(`Release lookup failed, linking to the releases page: ${error}`);
    return null;
  }
}
