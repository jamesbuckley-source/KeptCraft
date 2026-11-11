import assert from "node:assert/strict";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { startInstanceServer } from "../server.mjs";

async function withServer(run) {
  const workspaceRoot = await fs.mkdtemp(path.join(os.tmpdir(), "feature-tracker-server-test-"));
  const server = await startInstanceServer({ workspaceRoot, view: { status: "shipped" } });
  try {
    await run(workspaceRoot, server);
  } finally {
    await server.close();
    await fs.rm(workspaceRoot, { recursive: true, force: true });
  }
}

function endpoint(server, route) {
  return new URL(route, server.url);
}

async function json(response) {
  const body = await response.json();
  return { status: response.status, body };
}

function jsonRequest(method, body) {
  return {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}

test("serves the static page with a restrictive policy", async () => {
  await withServer(async (_workspaceRoot, server) => {
    const page = await fetch(server.url);
    assert.equal(page.status, 200);
    assert.match(page.headers.get("content-type"), /text\/html/);
    assert.match(page.headers.get("content-security-policy"), /script-src 'self'/);
    const html = await page.text();
    assert.match(html, /KeptCraft features/);
    assert.match(html, /id="editor"/);
    assert.match(html, /id="field-image"/);

    const script = await fetch(endpoint(server, "app.js"));
    assert.equal(script.status, 200);
    assert.match(await script.text(), /addFeature|renderCard/);

    const styles = await fetch(endpoint(server, "styles.css"));
    assert.equal(styles.status, 200);
    assert.match(styles.headers.get("content-type"), /text\/css/);
  });
});

test("reads state, creates, updates and removes features over HTTP", async () => {
  await withServer(async (workspaceRoot, server) => {
    const initial = await json(await fetch(endpoint(server, "api/state")));
    assert.equal(initial.status, 200);
    assert.equal(initial.body.missing, true);
    assert.equal(initial.body.summary.total, 0);
    assert.deepEqual(initial.body.view, { status: "shipped" });
    assert.deepEqual(initial.body.statuses, ["backlog", "planned", "in-progress", "blocked", "shipped"]);
    assert.equal(initial.body.statusLabels.backlog, "Backlog");
    assert.equal(initial.body.artifactPath, path.join(workspaceRoot, "docs", "features.json"));

    const created = await json(
      await fetch(endpoint(server, "api/features"), jsonRequest("POST", { title: "Tray menu", area: "Window" })),
    );
    assert.equal(created.status, 201);
    assert.equal(created.body.summary.total, 1);
    assert.equal(created.body.created.id, "tray-menu");
    assert.equal(created.body.created.status, "planned");

    const updated = await json(
      await fetch(endpoint(server, "api/features/tray-menu"), jsonRequest("PATCH", { status: "shipped" })),
    );
    assert.equal(updated.status, 200);
    assert.equal(updated.body.updated.status, "shipped");
    assert.equal(updated.body.summary.counts.shipped, 1);

    const removed = await json(await fetch(endpoint(server, "api/features/tray-menu"), { method: "DELETE" }));
    assert.equal(removed.status, 200);
    assert.equal(removed.body.summary.total, 0);

    const state = JSON.parse(await fs.readFile(path.join(workspaceRoot, "docs", "features.json"), "utf8"));
    assert.equal(state.format, "keptcraft-feature-tracker");
    assert.deepEqual(state.features, []);
  });
});

test("creates, serves and removes feature images over HTTP", async () => {
  await withServer(async (_workspaceRoot, server) => {
    const image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jlb8AAAAASUVORK5CYII=";
    const created = await json(
      await fetch(endpoint(server, "api/features"), jsonRequest("POST", { title: "Image card", image })),
    );
    assert.equal(created.status, 201);
    assert.equal(created.body.created.image, image);

    const updated = await json(
      await fetch(endpoint(server, "api/features/image-card"), jsonRequest("PATCH", { image: "" })),
    );
    assert.equal(updated.status, 200);
    assert.equal("image" in updated.body.updated, false);

    const unsupported = await json(
      await fetch(endpoint(server, "api/features"), jsonRequest("POST", {
        title: "Unsupported image",
        image: "data:image/svg+xml;base64,PHN2Zy8+",
      })),
    );
    assert.equal(unsupported.status, 400);
  });
});

test("rejects invalid input and unknown routes", async () => {
  await withServer(async (_workspaceRoot, server) => {
    const wrongType = await fetch(endpoint(server, "api/features"), {
      method: "POST",
      headers: { "Content-Type": "text/plain" },
      body: "hello",
    });
    assert.equal(wrongType.status, 415);

    const badJson = await fetch(endpoint(server, "api/features"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{ nope",
    });
    assert.equal(badJson.status, 400);

    const badStatus = await json(
      await fetch(endpoint(server, "api/features"), jsonRequest("POST", { title: "Thing", status: "done" })),
    );
    assert.equal(badStatus.status, 400);
    assert.equal(badStatus.body.error.code, "invalid-input");

    const missingTitle = await json(await fetch(endpoint(server, "api/features"), jsonRequest("POST", {})));
    assert.equal(missingTitle.status, 400);

    const notFound = await json(
      await fetch(endpoint(server, "api/features/nope"), jsonRequest("PATCH", { status: "shipped" })),
    );
    assert.equal(notFound.status, 404);
    assert.equal(notFound.body.error.code, "not-found");

    const badId = await json(
      await fetch(endpoint(server, "api/features/Not%20Kebab"), jsonRequest("PATCH", { status: "shipped" })),
    );
    assert.equal(badId.status, 400);
    assert.equal(badId.body.error.code, "invalid-id");

    const method = await fetch(endpoint(server, "api/features"), { method: "GET" });
    assert.equal(method.status, 405);
    assert.equal(method.headers.get("allow"), "POST");

    const unknown = await fetch(endpoint(server, "api/nope"));
    assert.equal(unknown.status, 404);
  });
});

test("pushes state events to connected pages", async () => {
  await withServer(async (_workspaceRoot, server) => {
    const controller = new AbortController();
    const events = await fetch(endpoint(server, "api/events"), { signal: controller.signal });
    assert.equal(events.status, 200);
    assert.match(events.headers.get("content-type"), /text\/event-stream/);

    const reader = events.body.getReader();
    const decoder = new TextDecoder();
    let buffered = "";
    const readUntil = async (needle) => {
      const deadline = Date.now() + 3000;
      while (!buffered.includes(needle)) {
        if (Date.now() > deadline) throw new Error(`Timed out waiting for ${needle}`);
        const { value, done } = await reader.read();
        if (done) throw new Error("Event stream closed early");
        buffered += decoder.decode(value, { stream: true });
      }
      buffered = "";
    };

    await readUntil("retry:");
    await fetch(endpoint(server, "api/features"), jsonRequest("POST", { title: "Broadcast me" }));
    await readUntil("event: state");

    controller.abort();
  });
});

test("surfaces corrupt artifacts without overwriting them", async () => {
  await withServer(async (workspaceRoot, server) => {
    const artifactPath = path.join(workspaceRoot, "docs", "features.json");
    await fs.mkdir(path.dirname(artifactPath), { recursive: true });
    await fs.writeFile(artifactPath, "{ definitely not json", "utf8");

    const state = await json(await fetch(endpoint(server, "api/state")));
    assert.equal(state.status, 500);
    assert.equal(state.body.error.code, "corrupt-data");
    assert.equal(state.body.artifactPath, artifactPath);

    const created = await json(
      await fetch(endpoint(server, "api/features"), jsonRequest("POST", { title: "Should not land" })),
    );
    assert.equal(created.status, 500);
    assert.equal(await fs.readFile(artifactPath, "utf8"), "{ definitely not json");
  });
});
