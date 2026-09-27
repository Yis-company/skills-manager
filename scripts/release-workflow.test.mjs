import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { prepareRelease, publishRelease, requiredCliAssets, requiredPlatforms, uploadAsset, validateRelease, verifyRelease } from "./release-workflow.mjs";

const response = (data, { status = 200, headers = {} } = {}) => ({
  ok: status >= 200 && status < 300,
  status,
  headers: new Headers(headers),
  json: async () => data,
  text: async () => typeof data === "string" ? data : JSON.stringify(data),
});

const draft = (id = 1) => ({ id, tag_name: "v1.41.1", draft: true, assets: [] });

test("prepares a missing release once and returns its ID", async () => {
  const calls = [];
  const fetchImpl = async (url, options = {}) => {
    calls.push({ url, options });
    if (options.method === "POST") return response(draft(42), { status: 201 });
    return response([]);
  };
  const release = await prepareRelease({ repo: "owner/repo", tag: "v1.41.1", token: "token", body: "notes", fetchImpl });
  assert.equal(release.id, 42);
  assert.equal(calls.length, 2);
  assert.equal(JSON.parse(calls[1].options.body).body, "notes");
});

test("reuses exactly one draft release", async () => {
  const fetchImpl = async () => response([draft(12)]);
  assert.equal((await prepareRelease({ repo: "owner/repo", tag: "v1.41.1", token: "token", fetchImpl })).id, 12);
});

test("rejects duplicate drafts and published releases", async () => {
  const duplicate = async () => response([draft(1), draft(2)]);
  await assert.rejects(prepareRelease({ repo: "owner/repo", tag: "v1.41.1", token: "token", fetchImpl: duplicate }), /Found 2 releases/);
  const published = async () => response([{ ...draft(), draft: false }]);
  await assert.rejects(prepareRelease({ repo: "owner/repo", tag: "v1.41.1", token: "token", fetchImpl: published }), /already published/);
});

test("follows release pagination and surfaces GitHub API failures", async () => {
  let calls = 0;
  const paginated = async () => {
    calls++;
    return calls === 1
      ? response([], { headers: { link: '<https://api.github.com/repos/owner/repo/releases?per_page=100&page=2>; rel="next"' } })
      : response([draft(9)]);
  };
  assert.equal((await prepareRelease({ repo: "owner/repo", tag: "v1.41.1", token: "token", fetchImpl: paginated })).id, 9);
  assert.equal(calls, 2);
  await assert.rejects(prepareRelease({
    repo: "owner/repo", tag: "v1.41.1", token: "token",
    fetchImpl: async () => response("forbidden", { status: 403 }),
  }), /failed \(403\)/);
});

function completeRelease() {
  return {
    ...draft(),
    assets: [
      { id: 77, name: "latest.json" },
      ...requiredCliAssets.map((name) => ({ name })),
      { name: "app.sig" },
      { name: "app.dmg" },
      { name: "app.app.tar.gz" },
    ],
  };
}

function completeMetadata() {
  return { platforms: Object.fromEntries(requiredPlatforms.map((platform) => [platform, { signature: "sig", url: "https://example.com/app" }])) };
}

test("accepts one complete draft with all five updater platforms and CLI assets", () => {
  assert.equal(validateRelease(completeRelease(), "v1.41.1", completeMetadata()), undefined);
});

test("rejects missing or duplicate latest.json", () => {
  const release = completeRelease();
  release.assets = release.assets.filter(({ name }) => name !== "latest.json");
  assert.throws(() => validateRelease(release, "v1.41.1", completeMetadata()), /exactly one latest.json/);
  release.assets.push({ name: "latest.json" }, { name: "latest.json" });
  assert.throws(() => validateRelease(release, "v1.41.1", completeMetadata()), /exactly one latest.json/);
});

test("rejects missing Linux ARM64 updater metadata and CLI asset", () => {
  const release = completeRelease();
  release.assets = release.assets.filter(({ name }) => name !== "skills-manager-cli-Linux-arm64");
  assert.throws(() => validateRelease(release, "v1.41.1", completeMetadata()), /skills-manager-cli-Linux-arm64/);
  release.assets.push({ name: "skills-manager-cli-Linux-arm64" });
  const metadata = completeMetadata();
  delete metadata.platforms["linux-aarch64"];
  assert.throws(() => validateRelease(release, "v1.41.1", metadata), /linux-aarch64/);
});

test("requires nonempty signatures and URLs for every platform", () => {
  const metadata = completeMetadata();
  metadata.platforms["linux-aarch64"].signature = " ";
  assert.throws(() => validateRelease(completeRelease(), "v1.41.1", metadata), /linux-aarch64/);
});

test("verification fetches the prepared release and its metadata by exact ID", async () => {
  const calls = [];
  const fetchImpl = async (url) => {
    calls.push(url);
    return calls.length === 1 ? response(completeRelease()) : response(completeMetadata());
  };
  await verifyRelease({ repo: "owner/repo", tag: "v1.41.1", releaseId: 42, token: "token", fetchImpl });
  assert.deepEqual(calls, [
    "https://api.github.com/repos/owner/repo/releases/42",
    "https://api.github.com/repos/owner/repo/releases/assets/77",
  ]);
});

test("verification rejects wrong-tag releases and API errors without another request", async () => {
  for (const result of [
    response({ ...completeRelease(), tag_name: "v1.41.0" }),
    response("unavailable", { status: 503 }),
  ]) {
    const calls = [];
    await assert.rejects(verifyRelease({
      repo: "owner/repo", tag: "v1.41.1", releaseId: 42, token: "token",
      fetchImpl: async (url) => { calls.push(url); return result; },
    }));
    assert.deepEqual(calls, ["https://api.github.com/repos/owner/repo/releases/42"]);
  }
});

test("CLI upload replaces an asset on the exact prepared release ID", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "release-upload-"));
  try {
    const file = path.join(directory, "cli");
    await writeFile(file, "cli bytes");
    const calls = [];
    const fetchImpl = async (url, options = {}) => {
      calls.push({ url, options });
      if (options.method === "DELETE") return response({});
      if (options.method === "POST") return response({ id: 78 }, { status: 201 });
      return response({ ...draft(42), assets: [{ id: 55, name: "cli" }] });
    };
    await uploadAsset({ repo: "owner/repo", tag: "v1.41.1", releaseId: 42, token: "token", path: file, name: "cli", fetchImpl });
    assert.deepEqual(calls.map(({ url, options }) => [url, options.method ?? "GET"]), [
      ["https://api.github.com/repos/owner/repo/releases/42", "GET"],
      ["https://api.github.com/repos/owner/repo/releases/assets/55", "DELETE"],
      ["https://uploads.github.com/repos/owner/repo/releases/42/assets?name=cli", "POST"],
    ]);
    assert.equal(Buffer.from(calls[2].options.body).toString(), "cli bytes");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("CLI upload reads bytes before deletion and rejects wrong tags or API errors before mutation", async () => {
  const calls = [];
  const existing = { ...draft(42), assets: [{ id: 55, name: "cli" }] };
  await assert.rejects(uploadAsset({
    repo: "owner/repo", tag: "v1.41.1", releaseId: 42, token: "token",
    path: "/no/such/cli", name: "cli",
    fetchImpl: async (url) => { calls.push(url); return response(existing); },
  }), /ENOENT/);
  assert.deepEqual(calls, []);

  const directory = await mkdtemp(path.join(os.tmpdir(), "release-upload-"));
  try {
    const file = path.join(directory, "cli");
    await writeFile(file, "cli bytes");
    for (const result of [
      response({ ...existing, tag_name: "v1.41.0" }),
      response("unavailable", { status: 503 }),
    ]) {
      const urls = [];
      await assert.rejects(uploadAsset({
        repo: "owner/repo", tag: "v1.41.1", releaseId: 42, token: "token",
        path: file, name: "cli",
        fetchImpl: async (url) => { urls.push(url); return result; },
      }));
      assert.deepEqual(urls, ["https://api.github.com/repos/owner/repo/releases/42"]);
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("publication patches only the prepared draft ID after checking its tag", async () => {
  const calls = [];
  const fetchImpl = async (url, options = {}) => {
    calls.push({ url, options });
    return options.method === "PATCH" ? response({ ...draft(42), draft: false }) : response(draft(42));
  };
  await publishRelease({ repo: "owner/repo", tag: "v1.41.1", releaseId: 42, token: "token", fetchImpl });
  assert.deepEqual(calls.map(({ url, options }) => [url, options.method ?? "GET"]), [
    ["https://api.github.com/repos/owner/repo/releases/42", "GET"],
    ["https://api.github.com/repos/owner/repo/releases/42", "PATCH"],
  ]);

  for (const result of [
    response({ ...draft(42), tag_name: "v1.41.0" }),
    response("unavailable", { status: 503 }),
  ]) {
    const attempted = [];
    await assert.rejects(publishRelease({
      repo: "owner/repo", tag: "v1.41.1", releaseId: 42, token: "token",
      fetchImpl: async (url, options = {}) => { attempted.push(options.method ?? "GET"); return result; },
    }));
    assert.deepEqual(attempted, ["GET"]);
  }
});
