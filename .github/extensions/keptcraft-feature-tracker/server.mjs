import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { once } from "node:events";
import path from "node:path";

import * as store from "./store.mjs";

const ASSET_DIR = new URL("./assets/", import.meta.url);
const ASSET_TYPES = new Map([
  ["index.html", "text/html; charset=utf-8"],
  ["app.js", "text/javascript; charset=utf-8"],
  ["styles.css", "text/css; charset=utf-8"],
]);
const MAX_BODY_BYTES = 3 * 1024 * 1024;
const STATUS_BY_CODE = new Map([
  ["invalid-input", 400],
  ["not-found", 404],
  ["conflict", 409],
  ["locked", 409],
  ["corrupt-data", 500],
  ["unrecognized-data", 500],
  ["unsupported-version", 500],
  ["invalid-data", 500],
  ["read-failed", 500],
  ["write-failed", 500],
  ["lock-failed", 500],
]);

const SECURITY_HEADERS = {
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Content-Security-Policy":
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'",
};

class HttpError extends Error {
  constructor(status, code, message) {
    super(message);
    this.name = "HttpError";
    this.status = status;
    this.code = code;
  }
}

function normalizePath(filePath) {
  const resolved = path.resolve(filePath);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

function sendJson(res, status, payload) {
  const body = JSON.stringify(payload);
  res.writeHead(status, {
    ...SECURITY_HEADERS,
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": Buffer.byteLength(body),
    "Cache-Control": "no-store",
  });
  res.end(body);
}

function sendText(res, status, contentType, body) {
  res.writeHead(status, {
    ...SECURITY_HEADERS,
    "Content-Type": contentType,
    "Content-Length": Buffer.byteLength(body),
    "Cache-Control": "no-store",
  });
  res.end(body);
}

function sendMethodNotAllowed(res, allowed) {
  res.setHeader("Allow", allowed.join(", "));
  sendJson(res, 405, { error: { code: "method-not-allowed", message: `Use ${allowed.join(" or ")}.` } });
}

async function readJsonBody(req) {
  const contentType = String(req.headers["content-type"] ?? "").toLowerCase();
  if (!contentType.includes("application/json")) {
    throw new HttpError(415, "unsupported-media-type", "Send the body as application/json.");
  }
  let size = 0;
  const chunks = [];
  for await (const chunk of req) {
    size += chunk.length;
    if (size > MAX_BODY_BYTES) {
      throw new HttpError(413, "payload-too-large", "Request body is too large.");
    }
    chunks.push(chunk);
  }
  const raw = Buffer.concat(chunks).toString("utf8").trim();
  if (!raw) return {};
  try {
    return JSON.parse(raw);
  } catch {
    throw new HttpError(400, "invalid-json", "Request body is not valid JSON.");
  }
}

function assertPlainObject(body) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    throw new HttpError(400, "invalid-input", "Request body must be a JSON object.");
  }
  return body;
}

export async function startInstanceServer({ workspaceRoot, view = {} }) {
  const artifactPath = store.artifactPathFor(workspaceRoot);
  const normalizedArtifactPath = normalizePath(artifactPath);
  const clients = new Set();
  const instance = { view: { ...view } };

  const broadcast = () => {
    for (const client of clients) {
      try {
        client.write("event: state\ndata: {}\n\n");
      } catch {
        clients.delete(client);
      }
    }
  };

  const unsubscribe = store.subscribeChanges(({ artifactPath: changedPath }) => {
    if (normalizePath(changedPath) === normalizedArtifactPath) broadcast();
  });

  async function statePayload() {
    const state = await store.readState(workspaceRoot);
    return {
      ...state,
      view: { ...instance.view },
      statuses: [...store.STATUSES],
      statusLabels: { ...store.STATUS_LABELS },
    };
  }

  function sendError(res, error) {
    if (res.headersSent) {
      try {
        res.end();
      } catch {
        // The response is already unusable.
      }
      return;
    }
    if (error instanceof HttpError) {
      sendJson(res, error.status, { error: { code: error.code, message: error.message } });
      return;
    }
    if (error instanceof store.StoreError) {
      sendJson(res, STATUS_BY_CODE.get(error.code) ?? 500, {
        error: { code: error.code, message: error.message },
        artifactPath,
      });
      return;
    }
    const message = error instanceof Error ? error.message : String(error);
    sendJson(res, 500, { error: { code: "internal-error", message }, artifactPath });
  }

  function openEvents(req, res) {
    res.writeHead(200, {
      ...SECURITY_HEADERS,
      "Content-Type": "text/event-stream; charset=utf-8",
      "Cache-Control": "no-store",
      Connection: "keep-alive",
    });
    res.write("retry: 2000\n\n");
    clients.add(res);
    const heartbeat = setInterval(() => {
      try {
        res.write(": keepalive\n\n");
      } catch {
        // `close` will remove the client.
      }
    }, 20000);
    heartbeat.unref?.();
    req.on("close", () => {
      clearInterval(heartbeat);
      clients.delete(res);
    });
  }

  async function sendAsset(res, name) {
    let body;
    try {
      body = await readFile(new URL(name, ASSET_DIR), "utf8");
    } catch (error) {
      throw new HttpError(500, "asset-missing", `Could not read ${name}: ${error.message}`);
    }
    sendText(res, 200, ASSET_TYPES.get(name) ?? "text/plain; charset=utf-8", body);
  }

  async function handleRequest(req, res) {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const { pathname } = url;

    if (req.method === "GET" && (pathname === "/" || pathname === "/index.html")) {
      await sendAsset(res, "index.html");
      return;
    }
    if (req.method === "GET" && pathname === "/app.js") {
      await sendAsset(res, "app.js");
      return;
    }
    if (req.method === "GET" && pathname === "/styles.css") {
      await sendAsset(res, "styles.css");
      return;
    }
    if (req.method === "GET" && pathname === "/api/state") {
      sendJson(res, 200, await statePayload());
      return;
    }
    if (req.method === "GET" && pathname === "/api/events") {
      openEvents(req, res);
      return;
    }
    if (pathname === "/api/features") {
      if (req.method !== "POST") {
        sendMethodNotAllowed(res, ["POST"]);
        return;
      }
      const body = assertPlainObject(await readJsonBody(req));
      const created = await store.addFeature(workspaceRoot, body);
      sendJson(res, 201, { ...(await statePayload()), created: created.feature });
      return;
    }

    const match = /^\/api\/features\/([^/]+)$/.exec(pathname);
    if (match) {
      let id;
      try {
        id = decodeURIComponent(match[1]);
      } catch {
        throw new HttpError(400, "invalid-id", "The feature id is not valid URL encoding.");
      }
      if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(id)) {
        throw new HttpError(400, "invalid-id", `"${id}" is not a valid feature id.`);
      }
      if (req.method === "PATCH" || req.method === "PUT") {
        const body = assertPlainObject(await readJsonBody(req));
        const updated = await store.updateFeature(workspaceRoot, { ...body, id });
        sendJson(res, 200, { ...(await statePayload()), updated: updated.feature });
        return;
      }
      if (req.method === "DELETE") {
        await store.removeFeature(workspaceRoot, id);
        sendJson(res, 200, await statePayload());
        return;
      }
      sendMethodNotAllowed(res, ["PATCH", "DELETE"]);
      return;
    }

    sendJson(res, 404, { error: { code: "not-found", message: "Not found." } });
  }

  const server = createServer((req, res) => {
    res.on("error", () => {});
    handleRequest(req, res).catch((error) => sendError(res, error));
  });

  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  const port = address && typeof address === "object" ? address.port : 0;

  return {
    url: `http://127.0.0.1:${port}/`,
    artifactPath,
    setView(nextView) {
      instance.view = { ...nextView };
      broadcast();
    },
    async close() {
      unsubscribe();
      for (const client of clients) {
        try {
          client.end();
        } catch {
          // Already closed.
        }
      }
      clients.clear();
      const closed = new Promise((resolve) => server.close(() => resolve()));
      server.closeAllConnections?.();
      await closed;
    },
  };
}
