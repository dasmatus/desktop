// Writes each channel's newest GitHub release into the published site, as
// updates/<channel>/<arch>/index: one line per file sysupdate asks for, its
// size in bytes, and the release asset URLs that make it up, in order.
//
//   SHA256SUMS 1096 https://github.com/<repo>/releases/download/<tag>/SHA256SUMS-x86_64
//   losos-desktop_..._usr-x86-64_<uuid>.raw.xz 2621440000 https://...part-00 https://...part-01
//
// GitHub Pages can only serve files, and sysupdate wants one URL with a
// SHA256SUMS and whole files under it, which a release is not: each
// architecture's manifest is SHA256SUMS-<arch>, and a file of 2 GiB or more
// is split into .part-NN assets (ci.yml, publish). So the image runs
// losos-update-source on 127.0.0.1 (nixos/modules/update-source.nix), which
// reads this index and streams the assets to sysupdate as the files it asked
// for. Nothing here is trusted: sysupdate checks SHA256SUMS's signature and
// every file's hash as it always does.
//
// The index is written when the site is built, from GitHub's API: by docs.yml
// on a change to the docs, and by ci.yml's publish job, which starts docs.yml
// once a release is up. Without GITHUB_REPOSITORY (a local `npm run build`),
// or for the offline copy, nothing is written.

const fs = require('node:fs/promises');
const path = require('node:path');

const ARCHES = ['x86_64', 'aarch64'];

// The channel a release belongs to, as ci.yml's publish job tags it: a
// nightly is a prerelease tagged nightly-<YYYYMMDD>T<HHMM>, and a release
// someone tagged by hand is the stable channel's.
function channel(release) {
  if (release.draft) return null;
  if (/^nightly-[0-9]{8}T[0-9]{4}$/.test(release.tag_name)) return 'nightly';
  return release.prerelease ? null : 'stable';
}

// Code point order, as `sort` would give strings, whatever the locale.
const byText = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

// The index of one architecture of a release, or null when the release has
// none of that architecture. Mirrors src/losos-installer/src/github.rs's
// `files`, which serves a release to the installer the same way.
function index(release, arch) {
  const manifest = `SHA256SUMS-${arch}`;
  const files = new Map();
  for (const asset of release.assets) {
    let name = asset.name;
    if (name === manifest) name = 'SHA256SUMS';
    else if (name === `${manifest}.gpg`) name = 'SHA256SUMS.gpg';
    // The other architecture's manifest.
    else if (name.startsWith('SHA256SUMS')) continue;
    else {
      const part = name.match(/^(.+)\.part-[0-9]+$/);
      if (part) name = part[1];
    }
    if (!files.has(name)) files.set(name, []);
    files.get(name).push(asset);
  }
  if (!files.has('SHA256SUMS') || !files.has('SHA256SUMS.gpg')) return null;
  const lines = [...files].sort(([a], [b]) => byText(a, b)).map(([name, assets]) => {
    // .part-00 before .part-01: same width, so text order is number order.
    assets.sort((a, b) => byText(a.name, b.name));
    const size = assets.reduce((sum, a) => sum + a.size, 0);
    return [name, size, ...assets.map((a) => a.browser_download_url)].join(' ');
  });
  return `${lines.join('\n')}\n`;
}

// Each channel's newest release that carries the architecture, newest first
// as the API lists them.
function pick(releases) {
  const out = {};
  for (const release of releases) {
    const name = channel(release);
    if (!name) continue;
    for (const arch of ARCHES) {
      const key = `${name}/${arch}`;
      if (out[key]) continue;
      const text = index(release, arch);
      if (text) out[key] = { tag: release.tag_name, text };
    }
  }
  return out;
}

async function releases(repository) {
  const api = process.env.GITHUB_API_URL || 'https://api.github.com';
  const headers = { accept: 'application/vnd.github+json' };
  if (process.env.GITHUB_TOKEN) headers.authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const response = await fetch(`${api}/repos/${repository}/releases?per_page=100`, { headers });
  if (!response.ok) throw new Error(`release-index: ${repository}'s releases: ${response.status}`);
  return response.json();
}

module.exports = () => ({
  name: 'release-index',
  async postBuild({ outDir }) {
    const repository = process.env.GITHUB_REPOSITORY;
    if (!repository || process.env.DOCS_OFFLINE) return;
    for (const [key, { tag, text }] of Object.entries(pick(await releases(repository)))) {
      const dir = path.join(outDir, 'updates', key);
      await fs.mkdir(dir, { recursive: true });
      await fs.writeFile(path.join(dir, 'index'), text);
      console.log(`release-index: ${key} is ${tag}`);
    }
  },
});

module.exports.pick = pick;
