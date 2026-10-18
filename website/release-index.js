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

// The channel a release belongs to: its cadence, which ci.yml's publish job
// puts in front of the version stamp of its tag. A nightly is a
// prerelease tagged nightly-<YYYYMMDD>T<HHMM>, and a weekly, monthly or
// yearly release is a full one tagged weekly-, monthly- or yearly- the same
// way (cadence.yml). Anything else, such as a release made by hand, is in
// no channel.
const CHANNELS = ['nightly', 'weekly', 'monthly', 'yearly'];

function channel(release) {
  if (release.draft) return null;
  const match = release.tag_name.match(/^([a-z]+)-[0-9]{8}T[0-9]{4}$/);
  return match && CHANNELS.includes(match[1]) ? match[1] : null;
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
      if (text) out[key] = { tag: release.tag_name, url: release.html_url, text };
    }
  }
  return out;
}

// Each channel's pick, fetched once per build for both this plugin and the
// flasher's config; empty without GITHUB_REPOSITORY.
let once;
function picked() {
  const repository = process.env.GITHUB_REPOSITORY;
  if (!repository || process.env.DOCS_OFFLINE) return Promise.resolve({});
  once ??= releases(repository).then(pick);
  return once;
}

// The newest release page of each channel that has the GSI's architecture,
// for the web flasher. Without a proxy it sends the person there, since a
// page cannot read release assets itself: GitHub serves them without CORS
// headers.
async function flasherReleases() {
  const out = {};
  for (const [key, { tag, url }] of Object.entries(await picked())) {
    const [name, arch] = key.split('/');
    if (arch === 'aarch64') out[name] = { tag, url };
  }
  return out;
}

// Every release, newest first, a page at a time. Nightlies come several a
// day, so a yearly release is soon hundreds of releases down the list; the
// pages stop once every channel has its pick on both architectures.
async function releases(repository) {
  const api = process.env.GITHUB_API_URL || 'https://api.github.com';
  const headers = { accept: 'application/vnd.github+json' };
  if (process.env.GITHUB_TOKEN) headers.authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const all = [];
  for (let page = 1; ; page++) {
    const url = `${api}/repos/${repository}/releases?per_page=100&page=${page}`;
    const response = await fetch(url, { headers });
    if (!response.ok) throw new Error(`release-index: ${repository}'s releases: ${response.status}`);
    const batch = await response.json();
    all.push(...batch);
    if (batch.length < 100) return all;
    if (Object.keys(pick(all)).length === CHANNELS.length * ARCHES.length) return all;
  }
}

module.exports = () => ({
  name: 'release-index',
  async postBuild({ outDir }) {
    for (const [key, { tag, text }] of Object.entries(await picked())) {
      const dir = path.join(outDir, 'updates', key);
      await fs.mkdir(dir, { recursive: true });
      await fs.writeFile(path.join(dir, 'index'), text);
      console.log(`release-index: ${key} is ${tag}`);
    }
  },
});

module.exports.pick = pick;
module.exports.channel = channel;
module.exports.CHANNELS = CHANNELS;
module.exports.flasherReleases = flasherReleases;
