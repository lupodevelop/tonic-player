// Points the download buttons at the files of the latest release. Without an
// answer from GitHub they keep their link to the release page.
type Asset = { name: string; browser_download_url: string };
type Links = Partial<Record<'mac' | 'macIntel' | 'win' | 'linux', string>>;

const PATTERNS: [keyof Links, RegExp][] = [
  ['mac', /_aarch64\.dmg$/],
  ['macIntel', /_x64\.dmg$/],
  ['win', /-setup\.exe$/],
  ['linux', /\.AppImage$/],
];

export function pickDownloads(assets: Asset[]): Links {
  const links: Links = {};
  for (const [os, pattern] of PATTERNS) {
    const asset = assets.find((a) => pattern.test(a.name));
    if (asset) links[os] = asset.browser_download_url;
  }
  return links;
}

/** `repo` is the GitHub page of the repository. */
export async function linkDownloads(repo: string) {
  const api = repo.replace('https://github.com/', 'https://api.github.com/repos/');
  try {
    const response = await fetch(`${api}/releases/latest`, { headers: { Accept: 'application/vnd.github+json' } });
    if (!response.ok) return;
    const links = pickDownloads((await response.json()).assets ?? []);
    for (const [os, url] of Object.entries(links)) {
      document.querySelectorAll<HTMLAnchorElement>(`[data-os="${os}"]`).forEach((a) => (a.href = url));
    }
  } catch {
    // Offline or rate limited: the release page still works.
  }
}
