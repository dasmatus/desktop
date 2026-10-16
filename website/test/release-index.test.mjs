// release-index.js against releases shaped the way ci.yml's publish job
// makes them: both architectures in one release, each manifest renamed by
// architecture, and a file of 2 GiB or more in parts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const { pick } = createRequire(import.meta.url)("../release-index.js");

const asset = (tag, name, size) => ({
  name,
  size,
  browser_download_url: `https://example.com/download/${tag}/${name}`,
});

const release = (tag, { prerelease = false, draft = false, files }) => ({
  tag_name: tag,
  prerelease,
  draft,
  assets: files.map(([name, size]) => asset(tag, name, size)),
});

const NIGHTLY = release("nightly-20261016T1200", {
  prerelease: true,
  files: [
    ["SHA256SUMS-x86_64", 10],
    ["SHA256SUMS-x86_64.gpg", 2],
    ["SHA256SUMS-aarch64", 11],
    ["SHA256SUMS-aarch64.gpg", 3],
    ["os_1_x86_64.efi", 5],
    ["os_1_usr-x86-64_u.raw.xz.part-01", 7],
    ["os_1_usr-x86-64_u.raw.xz.part-00", 100],
    ["os_1_aarch64.efi", 6],
  ],
});

test("a channel's index names each file once, its parts in order", () => {
  const out = pick([NIGHTLY]);
  assert.deepEqual(Object.keys(out).sort(), ["nightly/aarch64", "nightly/x86_64"]);
  const base = "https://example.com/download/nightly-20261016T1200";
  assert.equal(
    out["nightly/x86_64"].text,
    [
      `SHA256SUMS 10 ${base}/SHA256SUMS-x86_64`,
      `SHA256SUMS.gpg 2 ${base}/SHA256SUMS-x86_64.gpg`,
      `os_1_aarch64.efi 6 ${base}/os_1_aarch64.efi`,
      `os_1_usr-x86-64_u.raw.xz 107 ${base}/os_1_usr-x86-64_u.raw.xz.part-00 ${base}/os_1_usr-x86-64_u.raw.xz.part-01`,
      `os_1_x86_64.efi 5 ${base}/os_1_x86_64.efi`,
      "",
    ].join("\n"),
  );
  assert.match(out["nightly/aarch64"].text, /^SHA256SUMS 11 .*SHA256SUMS-aarch64$/m);
});

test("the newest release of each channel wins, drafts and odd prereleases never", () => {
  const older = release("nightly-20261015T0529", { prerelease: true, files: NIGHTLY.assets.map((a) => [a.name, 1]) });
  const stable = release("v0.1.0", { files: [["SHA256SUMS-x86_64", 1], ["SHA256SUMS-x86_64.gpg", 1]] });
  const draft = release("nightly-20261017T0000", { prerelease: true, draft: true, files: [["SHA256SUMS-x86_64", 1], ["SHA256SUMS-x86_64.gpg", 1]] });
  const beta = release("v0.2.0-rc1", { prerelease: true, files: [["SHA256SUMS-x86_64", 1], ["SHA256SUMS-x86_64.gpg", 1]] });
  const out = pick([draft, beta, NIGHTLY, older, stable]);
  assert.equal(out["nightly/x86_64"].tag, "nightly-20261016T1200");
  assert.equal(out["stable/x86_64"].tag, "v0.1.0");
  // v0.1.0 carries no aarch64, so the stable channel has none.
  assert.equal(out["stable/aarch64"], undefined);
});

test("a release without a signed manifest serves nothing", () => {
  const unsigned = release("v0.1.1", { files: [["SHA256SUMS-x86_64", 1], ["os_1_x86_64.efi", 1]] });
  assert.deepEqual(pick([unsigned]), {});
});
