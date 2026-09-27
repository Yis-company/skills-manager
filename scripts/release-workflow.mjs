import { pathToFileURL } from "node:url";

const api = "https://api.github.com";
const assertReleaseId = (id) => {
  if (!/^\d+$/.test(String(id))) throw new Error(`Invalid release ID: ${id}`);
};

export const requiredPlatforms = [
  "darwin-aarch64",
  "darwin-x86_64",
  "windows-x86_64",
  "linux-x86_64",
  "linux-aarch64",
];

export const requiredCliAssets = [
  "skills-manager-cli-macOS-arm64",
  "skills-manager-cli-macOS-x64",
  "skills-manager-cli-Windows-x64.exe",
  "skills-manager-cli-Linux-x64",
  "skills-manager-cli-Linux-arm64",
];

function headers(token, accept = "application/vnd.github+json") {
  return {
    authorization: `Bearer ${token}`,
    accept,
    "x-github-api-version": "2022-11-28",
  };
}

async function request(url, token, options = {}, fetchImpl = fetch) {
  const response = await fetchImpl(url, {
    ...options,
    headers: { ...headers(token), ...options.headers },
  });
  if (!response.ok) {
    throw new Error(`GitHub API ${options.method ?? "GET"} ${url} failed (${response.status}): ${await response.text()}`);
  }
  return response;
}

async function listReleases(repo, token, fetchImpl = fetch) {
  const releases = [];
  let url = `${api}/repos/${repo}/releases?per_page=100`;
  while (url) {
    const response = await request(url, token, {}, fetchImpl);
    releases.push(...await response.json());
    const next = response.headers.get("link")?.match(/<([^>]+)>; rel="next"/);
    url = next?.[1] ?? "";
  }
  return releases;
}

export async function prepareRelease({ repo, tag, token, body = "", fetchImpl = fetch }) {
  const matches = (await listReleases(repo, token, fetchImpl)).filter((release) => release.tag_name === tag);
  if (matches.length > 1) throw new Error(`Found ${matches.length} releases for ${tag}; resolve duplicates before building`);
  if (matches.length === 1) {
    if (!matches[0].draft) throw new Error(`Release ${tag} is already published; refusing to overwrite it`);
    assertReleaseId(matches[0].id);
    return matches[0];
  }
  const release = await (await request(`${api}/repos/${repo}/releases`, token, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ tag_name: tag, name: `Skills Manager ${tag}`, body, draft: true, prerelease: false }),
  }, fetchImpl)).json();
  assertReleaseId(release.id);
  if (release.tag_name !== tag || !release.draft) throw new Error(`GitHub did not create the expected draft release for ${tag}`);
  return release;
}

export function validateRelease(release, tag, latest) {
  if (release.tag_name !== tag) throw new Error(`Release ID ${release.id} belongs to ${release.tag_name}, expected ${tag}`);
  if (!release.draft) throw new Error(`Release ${tag} is already published; refusing to validate it for overwrite`);

  const assets = release.assets ?? [];
  const names = assets.map(({ name }) => name);
  const latestAssets = assets.filter(({ name }) => name === "latest.json");
  if (latestAssets.length !== 1) throw new Error(`Expected exactly one latest.json asset, found ${latestAssets.length}`);
  const missingCli = requiredCliAssets.filter((name) => !names.includes(name));
  if (missingCli.length) throw new Error(`Missing CLI release assets: ${missingCli.join(", ")}`);
  if (!names.some((name) => name.endsWith(".sig"))) throw new Error("Missing updater signature assets (*.sig)");
  for (const pattern of [/\.dmg$/, /\.app\.tar\.gz$/]) {
    if (!names.some((name) => pattern.test(name))) throw new Error(`Missing macOS asset matching ${pattern}`);
  }
  for (const platform of requiredPlatforms) {
    const entry = latest?.platforms?.[platform];
    if (typeof entry?.signature !== "string" || !entry.signature.trim() || typeof entry?.url !== "string" || !entry.url.trim()) {
      throw new Error(`latest.json is missing a signed updater entry for ${platform}`);
    }
  }
}

export async function verifyRelease({ repo, tag, releaseId, token, fetchImpl = fetch }) {
  assertReleaseId(releaseId);
  const release = await (await request(`${api}/repos/${repo}/releases/${releaseId}`, token, {}, fetchImpl)).json();
  if (release.tag_name !== tag) throw new Error(`Release ID ${releaseId} belongs to ${release.tag_name}, expected ${tag}`);
  const metadata = (release.assets ?? []).filter(({ name }) => name === "latest.json");
  if (metadata.length !== 1) throw new Error(`Expected exactly one latest.json asset, found ${metadata.length}`);
  const latestResponse = await request(`${api}/repos/${repo}/releases/assets/${metadata[0].id}`, token, {
    headers: { accept: "application/octet-stream" },
  }, fetchImpl);
  const latest = JSON.parse(await latestResponse.text());
  validateRelease(release, tag, latest);
}

export async function uploadAsset({ repo, tag, releaseId, token, path, name, fetchImpl = fetch }) {
  assertReleaseId(releaseId);
  const bytes = await (await import("node:fs/promises")).readFile(path);
  const release = await (await request(`${api}/repos/${repo}/releases/${releaseId}`, token, {}, fetchImpl)).json();
  if (release.tag_name !== tag) throw new Error(`Release ID ${releaseId} belongs to ${release.tag_name}, expected ${tag}`);
  if (!release.draft) throw new Error(`Release ${releaseId} is already published; refusing asset upload`);
  const existing = (release.assets ?? []).find((asset) => asset.name === name);
  if (existing) await request(`${api}/repos/${repo}/releases/assets/${existing.id}`, token, { method: "DELETE" }, fetchImpl);
  const url = `https://uploads.github.com/repos/${repo}/releases/${releaseId}/assets?name=${encodeURIComponent(name)}`;
  await request(url, token, {
    method: "POST",
    headers: { "content-type": "application/octet-stream" },
    body: bytes,
  }, fetchImpl);
}

export async function publishRelease({ repo, tag, releaseId, token, fetchImpl = fetch }) {
  assertReleaseId(releaseId);
  const current = await (await request(`${api}/repos/${repo}/releases/${releaseId}`, token, {}, fetchImpl)).json();
  if (current.tag_name !== tag) throw new Error(`Release ID ${releaseId} belongs to ${current.tag_name}, expected ${tag}`);
  if (!current.draft) throw new Error(`Release ${tag} is already published; refusing to publish it again`);
  const release = await (await request(`${api}/repos/${repo}/releases/${releaseId}`, token, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ draft: false }),
  }, fetchImpl)).json();
  if (release.tag_name !== tag || release.draft) throw new Error(`GitHub did not publish the prepared ${tag} release (${releaseId})`);
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  const { GITHUB_TOKEN, GITHUB_REPOSITORY, GITHUB_REF_NAME, RELEASE_ID, GITHUB_OUTPUT } = process.env;
  if (command === "prepare") {
    const release = await prepareRelease({ repo: GITHUB_REPOSITORY, tag: GITHUB_REF_NAME, token: GITHUB_TOKEN, body: process.env.RELEASE_BODY });
    await (await import("node:fs/promises")).appendFile(GITHUB_OUTPUT, `release_id=${release.id}\n`);
    console.log(`Prepared draft release ${release.id} for ${GITHUB_REF_NAME}`);
  } else if (command === "verify") {
    await verifyRelease({ repo: GITHUB_REPOSITORY, tag: GITHUB_REF_NAME, releaseId: RELEASE_ID, token: GITHUB_TOKEN });
    console.log(`Updater assets validation passed for release ${RELEASE_ID}`);
  } else if (command === "upload") {
    const [path, name] = args;
    await uploadAsset({ repo: GITHUB_REPOSITORY, tag: GITHUB_REF_NAME, releaseId: RELEASE_ID, token: GITHUB_TOKEN, path, name });
  } else if (command === "publish") {
    await publishRelease({ repo: GITHUB_REPOSITORY, tag: GITHUB_REF_NAME, releaseId: RELEASE_ID, token: GITHUB_TOKEN });
    console.log(`Published ${GITHUB_REF_NAME} (release ${RELEASE_ID})`);
  } else {
    throw new Error(`Unknown command: ${command}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
