import assert from "node:assert/strict";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import * as store from "../store.mjs";

async function withWorkspace(run) {
  const workspaceRoot = await fs.mkdtemp(path.join(os.tmpdir(), "feature-tracker-test-"));
  try {
    await run(workspaceRoot, store.artifactPathFor(workspaceRoot));
  } finally {
    await fs.rm(workspaceRoot, { recursive: true, force: true });
  }
}

test("a missing artifact reads as an empty board", async () => {
  await withWorkspace(async (workspaceRoot, artifactPath) => {
    const state = await store.readState(workspaceRoot);
    assert.equal(state.missing, true);
    assert.equal(state.artifactPath, artifactPath);
    assert.equal(state.summary.total, 0);
    assert.deepEqual(state.features, []);
    await assert.rejects(fs.stat(artifactPath), { code: "ENOENT" });
  });
});

test("adding a feature writes a versioned artifact with sensible defaults", async () => {
  await withWorkspace(async (workspaceRoot, artifactPath) => {
    const added = await store.addFeature(workspaceRoot, { title: "  Capture text clips  " });
    assert.equal(added.feature.id, "capture-text-clips");
    assert.equal(added.feature.title, "Capture text clips");
    assert.equal(added.feature.area, "General");
    assert.equal(added.feature.status, "planned");
    assert.deepEqual(added.feature.references, []);

    const parsed = JSON.parse(await fs.readFile(artifactPath, "utf8"));
    assert.equal(parsed.format, store.FORMAT);
    assert.equal(parsed.version, store.VERSION);
    assert.equal(parsed.features.length, 1);
    assert.match(parsed.updatedAt, /^\d{4}-\d{2}-\d{2}T/);
  });
});

test("generated ids stay unique and explicit ids are validated", async () => {
  await withWorkspace(async (workspaceRoot) => {
    const first = await store.addFeature(workspaceRoot, { title: "Search ranking", status: "shipped" });
    const second = await store.addFeature(workspaceRoot, { title: "Search ranking" });
    assert.equal(first.feature.id, "search-ranking");
    assert.equal(second.feature.id, "search-ranking-2");

    await assert.rejects(store.addFeature(workspaceRoot, { title: "Duplicate", id: "search-ranking" }), {
      code: "conflict",
    });
    await assert.rejects(store.addFeature(workspaceRoot, { title: "Bad id", id: "Not Kebab" }), {
      code: "invalid-input",
    });
  });
});

test("generated ids stay kebab-case for titles with special characters", async () => {
  await withWorkspace(async (workspaceRoot) => {
    const kebab = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
    const truncated = await store.addFeature(workspaceRoot, {
      title: `${"a".repeat(59)} \u2014 \u201cquoted\u201d \u{1f389}`,
    });
    assert.match(truncated.feature.id, kebab);

    const symbolsOnly = await store.addFeature(workspaceRoot, { title: "\u2014 \u201c\u201d \u{1f389} !!!" });
    assert.equal(symbolsOnly.feature.id, "feature");

    const state = await store.readState(workspaceRoot);
    assert.equal(state.summary.total, 2);
  });
});

test("updating a feature moves it between statuses and records the change", async () => {
  await withWorkspace(async (workspaceRoot) => {
    await store.addFeature(workspaceRoot, { title: "Paste back", area: "Integration" });
    const updated = await store.updateFeature(workspaceRoot, { id: "paste-back", status: "in-progress" });
    assert.equal(updated.feature.status, "in-progress");
    assert.equal(updated.summary.counts["in-progress"], 1);

    const state = await store.readState(workspaceRoot);
    assert.equal(state.features[0].status, "in-progress");
    assert.equal(state.summary.shippedPercent, 0);

    await assert.rejects(store.updateFeature(workspaceRoot, { id: "paste-back", status: "done" }), {
      code: "invalid-input",
    });
    await assert.rejects(store.updateFeature(workspaceRoot, { id: "missing-feature", status: "shipped" }), {
      code: "not-found",
    });
    await assert.rejects(store.updateFeature(workspaceRoot, { id: "paste-back" }), { code: "invalid-input" });
  });
});

test("images are validated, persisted, replaced and removable", async () => {
  await withWorkspace(async (workspaceRoot) => {
    const image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jlb8AAAAASUVORK5CYII=";
    const added = await store.addFeature(workspaceRoot, { title: "Illustrated card", image });
    assert.equal(added.feature.image, image);
    assert.equal((await store.readState(workspaceRoot)).features[0].image, image);

    await assert.rejects(store.addFeature(workspaceRoot, { title: "SVG", image: "data:image/svg+xml;base64,PHN2Zy8+" }), {
      code: "invalid-input",
    });
    await assert.rejects(
      store.addFeature(workspaceRoot, { title: "Mismatched image", image: "data:image/jpeg;base64,aGVsbG8=" }),
      { code: "invalid-input" },
    );
    const oversizedImage = Buffer.alloc(2 * 1024 * 1024 + 1);
    oversizedImage.set([137, 80, 78, 71, 13, 10, 26, 10]);
    await assert.rejects(
      store.addFeature(workspaceRoot, { title: "Oversized image", image: `data:image/png;base64,${oversizedImage.toString("base64")}` }),
      { code: "invalid-input" },
    );

    const updated = await store.updateFeature(workspaceRoot, { id: added.feature.id, image: "" });
    assert.equal("image" in updated.feature, false);
  });
});

test("backlog features are accepted and sort before planned ones", async () => {
  await withWorkspace(async (workspaceRoot) => {
    await store.addFeature(workspaceRoot, { title: "Deferred sync", status: "backlog" });
    await store.addFeature(workspaceRoot, { title: "Agreed next", status: "planned" });

    const state = await store.readState(workspaceRoot);
    assert.equal(state.summary.counts.backlog, 1);
    assert.deepEqual(
      state.features.map((feature) => feature.status),
      ["backlog", "planned"],
    );
    assert.deepEqual(
      (await store.listFeatures(workspaceRoot, { status: "backlog" })).features.map((feature) => feature.id),
      ["deferred-sync"],
    );
  });
});

test("removing a feature updates the summary", async () => {
  await withWorkspace(async (workspaceRoot) => {
    await store.addFeature(workspaceRoot, { title: "Tray menu", status: "shipped" });
    const removed = await store.removeFeature(workspaceRoot, "tray-menu");
    assert.equal(removed.removed.id, "tray-menu");
    assert.equal(removed.summary.total, 0);
    await assert.rejects(store.removeFeature(workspaceRoot, "tray-menu"), { code: "not-found" });
  });
});

test("filters match status, area and free text", async () => {
  await withWorkspace(async (workspaceRoot) => {
    await store.addFeature(workspaceRoot, {
      title: "Encrypt clips",
      area: "Security",
      status: "shipped",
      references: ["src-tauri/src/crypto.rs"],
    });
    await store.addFeature(workspaceRoot, { title: "Search ranking", area: "Search", status: "planned" });

    assert.equal((await store.listFeatures(workspaceRoot, { status: "shipped" })).matched, 1);
    assert.equal((await store.listFeatures(workspaceRoot, { area: "search" })).matched, 1);
    assert.equal((await store.listFeatures(workspaceRoot, { query: "crypto.rs" })).matched, 1);
    assert.equal((await store.listFeatures(workspaceRoot, {})).matched, 2);
    await assert.rejects(store.listFeatures(workspaceRoot, { status: "done" }), { code: "invalid-input" });
  });
});

test("corrupt and foreign artifacts are reported, never overwritten", async () => {
  await withWorkspace(async (workspaceRoot, artifactPath) => {
    await fs.mkdir(path.dirname(artifactPath), { recursive: true });
    await fs.writeFile(artifactPath, "{ not json", "utf8");
    await assert.rejects(store.readState(workspaceRoot), { code: "corrupt-data" });
    await assert.rejects(store.addFeature(workspaceRoot, { title: "Anything" }), { code: "corrupt-data" });
    assert.equal(await fs.readFile(artifactPath, "utf8"), "{ not json");

    await fs.writeFile(artifactPath, JSON.stringify({ format: "something-else", version: 1, features: [] }), "utf8");
    await assert.rejects(store.readState(workspaceRoot), { code: "unrecognized-data" });
    assert.match(await fs.readFile(artifactPath, "utf8"), /something-else/);

    await fs.writeFile(artifactPath, JSON.stringify({ format: store.FORMAT, version: 99, features: [] }), "utf8");
    await assert.rejects(store.readState(workspaceRoot), { code: "unsupported-version" });

    await fs.writeFile(
      artifactPath,
      JSON.stringify({ format: store.FORMAT, version: 1, features: [{ id: "x", title: "X", status: "odd" }] }),
      "utf8",
    );
    await assert.rejects(store.readState(workspaceRoot), { code: "invalid-data" });
  });
});

test("parallel writes are serialized so none are lost", async () => {
  await withWorkspace(async (workspaceRoot) => {
    const additions = Array.from({ length: 8 }, (_, index) =>
      store.addFeature(workspaceRoot, { title: `Feature ${index + 1}`, status: "planned" }),
    );
    await Promise.all(additions);
    const state = await store.readState(workspaceRoot);
    assert.equal(state.summary.total, 8);
    assert.equal(new Set(state.features.map((feature) => feature.id)).size, 8);
  });
});

test("references are trimmed and de-duplicated, and invalid entries are rejected", async () => {
  await withWorkspace(async (workspaceRoot) => {
    const added = await store.addFeature(workspaceRoot, {
      title: "Preview clips",
      references: ["  src/App.tsx ", "src/App.tsx", ""],
    });
    assert.deepEqual(added.feature.references, ["src/App.tsx"]);

    await assert.rejects(store.addFeature(workspaceRoot, { title: "Bad reference", references: [42] }), {
      code: "invalid-input",
    });

    const tooMany = Array.from({ length: 31 }, (_, index) => `file-${index}.ts`);
    await assert.rejects(store.addFeature(workspaceRoot, { title: "Too many", references: tooMany }), {
      code: "invalid-input",
    });
  });
});
