import { promises as fs, existsSync } from "node:fs";
import { createHash, randomUUID } from "node:crypto";
import os from "node:os";
import path from "node:path";

export const FORMAT = "keptcraft-feature-tracker";
export const VERSION = 1;
export const STATUSES = Object.freeze(["backlog", "planned", "in-progress", "blocked", "shipped"]);
export const STATUS_LABELS = Object.freeze({
  backlog: "Backlog",
  planned: "Planned",
  "in-progress": "In progress",
  blocked: "Blocked",
  shipped: "Shipped",
});
export const ARTIFACT_RELATIVE_PATH = "docs/features.json";

const ID_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const MAX_TITLE = 200;
const MAX_AREA = 60;
const MAX_SUMMARY = 2000;
const MAX_OWNER = 80;
const MAX_REFERENCE = 300;
const MAX_REFERENCES = 30;
const MAX_ID = 80;
const MAX_IMAGE_BYTES = 2 * 1024 * 1024;

const LOCK_ROOT = path.join(os.tmpdir(), "keptcraft-feature-tracker-locks");
const LOCK_WAIT_MS = 5000;
const LOCK_STALE_MS = 15000;
const LOCK_RETRY_MS = 40;

const STATUS_ORDER = new Map(STATUSES.map((status, index) => [status, index]));
const changeListeners = new Set();

export class StoreError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "StoreError";
    this.code = code;
  }
}

function invalid(message) {
  throw new StoreError("invalid-input", message);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function resolveWorkspaceRoot(ctx) {
  const fromSession = ctx?.session?.workingDirectory;
  if (typeof fromSession === "string" && fromSession.trim()) {
    return path.resolve(fromSession);
  }
  const start = process.cwd();
  let dir = start;
  for (let depth = 0; depth < 40; depth += 1) {
    if (existsSync(path.join(dir, ".git")) || existsSync(path.join(dir, "package.json"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return start;
}

export function artifactPathFor(workspaceRoot) {
  return path.resolve(workspaceRoot, "docs", "features.json");
}

export function slugify(value) {
  const slug = String(value ?? "")
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60)
    .replace(/-+$/g, "");
  return slug || "feature";
}

function uniqueId(features, base) {
  const taken = new Set(features.map((feature) => feature.id));
  if (!taken.has(base)) return base;
  let suffix = 2;
  while (taken.has(`${base}-${suffix}`)) suffix += 1;
  return `${base}-${suffix}`;
}

function textField(value, field, { max, required = false }) {
  if (value === undefined || value === null) {
    if (required) invalid(`${field} is required.`);
    return undefined;
  }
  if (typeof value !== "string") invalid(`${field} must be a string.`);
  const trimmed = value.trim();
  if (required && !trimmed) invalid(`${field} must not be empty.`);
  if (trimmed.length > max) invalid(`${field} must be at most ${max} characters.`);
  return trimmed;
}

function referencesField(value) {
  if (value === undefined) return undefined;
  if (value === null) return [];
  if (!Array.isArray(value)) invalid("references must be an array of strings.");
  if (value.length > MAX_REFERENCES) invalid(`references must contain at most ${MAX_REFERENCES} entries.`);
  const references = [];
  for (const entry of value) {
    if (typeof entry !== "string") invalid("references must be an array of strings.");
    const trimmed = entry.trim();
    if (!trimmed) continue;
    if (trimmed.length > MAX_REFERENCE) invalid(`each reference must be at most ${MAX_REFERENCE} characters.`);
    if (!references.includes(trimmed)) references.push(trimmed);
  }
  return references;
}

function imageField(value, code = "invalid-input") {
  if (value === undefined) return undefined;
  if (value === null || value === "") return "";
  const match = typeof value === "string"
    ? /^data:image\/(png|jpeg|gif|webp);base64,([A-Za-z0-9+/]+={0,2})$/.exec(value)
    : null;
  if (!match) throw new StoreError(code, "image must be a PNG, JPEG, GIF or WebP data URL.");

  const bytes = Buffer.from(match[2], "base64");
  if (bytes.length > MAX_IMAGE_BYTES) throw new StoreError(code, "image must be 2 MB or smaller.");
  if (!bytes.length || bytes.toString("base64") !== match[2]) {
    throw new StoreError(code, "image must contain valid base64 data.");
  }
  const signatures = {
    png: bytes.length >= 8 && bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])),
    jpeg: bytes.length >= 3 && bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255,
    gif: bytes.length >= 6 && /^GIF8[79]a$/.test(bytes.subarray(0, 6).toString("ascii")),
    webp: bytes.length >= 12 && bytes.toString("ascii", 0, 4) === "RIFF" && bytes.toString("ascii", 8, 12) === "WEBP",
  };
  if (!signatures[match[1]]) throw new StoreError(code, "image data does not match its declared format.");
  return value;
}

function parseStatus(value, field = "status") {
  if (typeof value !== "string" || !STATUSES.includes(value)) {
    invalid(`${field} must be one of: ${STATUSES.join(", ")}.`);
  }
  return value;
}

function emptyDocument() {
  return { format: FORMAT, version: VERSION, title: "KeptCraft features", features: [] };
}

function normalizeFeature(raw, source) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new StoreError("invalid-data", `${source} must be an object.`);
  }
  const id = typeof raw.id === "string" ? raw.id.trim() : "";
  if (!ID_PATTERN.test(id)) {
    throw new StoreError("invalid-data", `${source}.id must be kebab-case (a-z, 0-9, dashes).`);
  }
  const title = typeof raw.title === "string" ? raw.title.trim() : "";
  if (!title) {
    throw new StoreError("invalid-data", `${source}.title must be a non-empty string.`);
  }
  if (!STATUSES.includes(raw.status)) {
    throw new StoreError("invalid-data", `${source}.status must be one of: ${STATUSES.join(", ")}.`);
  }
  return {
    ...raw,
    id,
    title,
    area: typeof raw.area === "string" && raw.area.trim() ? raw.area.trim() : "General",
    status: raw.status,
    summary: typeof raw.summary === "string" ? raw.summary : "",
    owner: typeof raw.owner === "string" ? raw.owner : "",
    references: Array.isArray(raw.references)
      ? raw.references.filter((entry) => typeof entry === "string").map((entry) => entry.trim()).filter(Boolean)
      : [],
    ...(raw.image === undefined ? {} : { image: imageField(raw.image, "invalid-data") }),
    updatedAt: typeof raw.updatedAt === "string" ? raw.updatedAt : "",
  };
}

export function validateDocument(candidate, source) {
  if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) {
    throw new StoreError("invalid-data", `${source}: expected a JSON object at the top level.`);
  }
  if (candidate.format !== FORMAT) {
    throw new StoreError(
      "unrecognized-data",
      `${source}: "format" is ${JSON.stringify(candidate.format ?? null)}; expected "${FORMAT}". Refusing to touch a file this canvas does not own.`,
    );
  }
  if (candidate.version !== VERSION) {
    throw new StoreError(
      "unsupported-version",
      `${source}: format version ${JSON.stringify(candidate.version ?? null)} is not supported (this canvas reads version ${VERSION}).`,
    );
  }
  if (!Array.isArray(candidate.features)) {
    throw new StoreError("invalid-data", `${source}: "features" must be an array.`);
  }
  const features = candidate.features.map((feature, index) => normalizeFeature(feature, `${source}: features[${index}]`));
  const ids = new Set();
  for (const feature of features) {
    if (ids.has(feature.id)) {
      throw new StoreError("invalid-data", `${source}: duplicate feature id "${feature.id}".`);
    }
    ids.add(feature.id);
  }
  return {
    ...candidate,
    format: FORMAT,
    version: VERSION,
    title: typeof candidate.title === "string" && candidate.title.trim() ? candidate.title.trim() : "KeptCraft features",
    updatedAt: typeof candidate.updatedAt === "string" ? candidate.updatedAt : "",
    features,
  };
}

export async function readDocument(filePath) {
  let raw;
  try {
    raw = await fs.readFile(filePath, "utf8");
  } catch (error) {
    if (error && error.code === "ENOENT") {
      return { document: emptyDocument(), missing: true };
    }
    throw new StoreError("read-failed", `Could not read ${filePath}: ${error.message}`);
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new StoreError(
      "corrupt-data",
      `${filePath} is not valid JSON (${error.message}). Fix or delete the file; the canvas will not overwrite it.`,
    );
  }
  return { document: validateDocument(parsed, filePath), missing: false };
}

async function isStaleLock(lockPath) {
  try {
    const stats = await fs.stat(lockPath);
    return Date.now() - stats.mtimeMs > LOCK_STALE_MS;
  } catch {
    return false;
  }
}

async function acquireLock(lockPath) {
  try {
    const handle = await fs.open(lockPath, "wx");
    await handle.writeFile(`${process.pid}\n`, "utf8");
    return handle;
  } catch (error) {
    if (error && error.code === "EEXIST") return null;
    throw new StoreError("lock-failed", `Could not create ${lockPath}: ${error.message}`);
  }
}

export async function withDocumentLock(filePath, fn) {
  const key = createHash("sha256").update(path.resolve(filePath).toLowerCase()).digest("hex").slice(0, 20);
  await fs.mkdir(LOCK_ROOT, { recursive: true }).catch(() => {});
  const lockPath = path.join(LOCK_ROOT, `${key}.lock`);
  const deadline = Date.now() + LOCK_WAIT_MS;
  let handle = await acquireLock(lockPath);
  while (!handle) {
    if (Date.now() > deadline) {
      if (await isStaleLock(lockPath)) {
        await fs.rm(lockPath, { force: true }).catch(() => {});
      } else {
        throw new StoreError("locked", "Another KeptCraft feature-tracker session is writing the board. Try again.");
      }
    }
    await delay(LOCK_RETRY_MS);
    handle = await acquireLock(lockPath);
  }
  try {
    return await fn();
  } finally {
    await handle.close().catch(() => {});
    await fs.rm(lockPath, { force: true }).catch(() => {});
  }
}

export function subscribeChanges(listener) {
  changeListeners.add(listener);
  return () => changeListeners.delete(listener);
}

function notifyChange(filePath, document) {
  for (const listener of changeListeners) {
    try {
      listener({ artifactPath: filePath, document });
    } catch {
      // A failing listener must never break a successful write.
    }
  }
}

export async function writeDocument(filePath, document) {
  const next = { ...document, format: FORMAT, version: VERSION, updatedAt: new Date().toISOString() };
  const validated = validateDocument(next, filePath);
  const directory = path.dirname(filePath);
  await fs.mkdir(directory, { recursive: true });
  const tempPath = path.join(directory, `.${path.basename(filePath)}.${process.pid}.${randomUUID()}.tmp`);
  try {
    await fs.writeFile(tempPath, `${JSON.stringify(validated, null, 2)}\n`, "utf8");
    await fs.rename(tempPath, filePath);
  } catch (error) {
    await fs.rm(tempPath, { force: true }).catch(() => {});
    throw new StoreError("write-failed", `Could not write ${filePath}: ${error.message}`);
  }
  notifyChange(filePath, validated);
  return validated;
}

function sortFeatures(features) {
  return [...features].sort(
    (a, b) =>
      (STATUS_ORDER.get(a.status) ?? 99) - (STATUS_ORDER.get(b.status) ?? 99) ||
      String(b.updatedAt).localeCompare(String(a.updatedAt)) ||
      a.title.localeCompare(b.title),
  );
}

export function summarizeDocument(document) {
  const counts = Object.fromEntries(STATUSES.map((status) => [status, 0]));
  const areas = new Map();
  for (const feature of document.features) {
    counts[feature.status] += 1;
    let entry = areas.get(feature.area);
    if (!entry) {
      entry = { area: feature.area, total: 0, counts: Object.fromEntries(STATUSES.map((status) => [status, 0])) };
      areas.set(feature.area, entry);
    }
    entry.counts[feature.status] += 1;
    entry.total += 1;
  }
  const total = document.features.length;
  return {
    total,
    counts,
    shippedPercent: total === 0 ? 0 : Math.round((counts.shipped / total) * 100),
    areas: [...areas.values()].sort((a, b) => b.total - a.total || a.area.localeCompare(b.area)),
  };
}

export async function readState(workspaceRoot) {
  const artifactPath = artifactPathFor(workspaceRoot);
  const { document, missing } = await readDocument(artifactPath);
  return {
    artifactPath,
    artifactRelativePath: ARTIFACT_RELATIVE_PATH,
    missing,
    title: document.title,
    updatedAt: document.updatedAt || null,
    features: sortFeatures(document.features),
    summary: summarizeDocument(document),
  };
}

export async function listFeatures(workspaceRoot, filter = {}) {
  if (!filter || typeof filter !== "object" || Array.isArray(filter)) {
    invalid("Filters must be an object.");
  }
  const status = filter.status === undefined ? undefined : parseStatus(filter.status);
  const area = filter.area === undefined ? undefined : textField(filter.area, "area", { max: MAX_AREA });
  const query = filter.query === undefined ? undefined : textField(filter.query, "query", { max: 200 });
  const state = await readState(workspaceRoot);
  let features = state.features;
  if (status) features = features.filter((feature) => feature.status === status);
  if (area) features = features.filter((feature) => feature.area.toLowerCase() === area.toLowerCase());
  if (query) {
    const needle = query.toLowerCase();
    features = features.filter((feature) =>
      [feature.id, feature.title, feature.area, feature.summary, feature.owner, ...feature.references]
        .join("\n")
        .toLowerCase()
        .includes(needle),
    );
  }
  return { ...state, features, matched: features.length };
}

export async function addFeature(workspaceRoot, input = {}) {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    invalid("Feature input must be an object.");
  }
  const title = textField(input.title, "title", { max: MAX_TITLE, required: true });
  const area = textField(input.area, "area", { max: MAX_AREA }) || "General";
  const status = input.status === undefined ? "planned" : parseStatus(input.status);
  const summary = textField(input.summary, "summary", { max: MAX_SUMMARY }) ?? "";
  const owner = textField(input.owner, "owner", { max: MAX_OWNER }) ?? "";
  const references = referencesField(input.references) ?? [];
  const image = imageField(input.image);
  const requestedId = textField(input.id, "id", { max: MAX_ID });
  if (requestedId !== undefined && !ID_PATTERN.test(requestedId)) {
    invalid("id must be kebab-case (a-z, 0-9, dashes).");
  }

  const artifactPath = artifactPathFor(workspaceRoot);
  return withDocumentLock(artifactPath, async () => {
    const { document } = await readDocument(artifactPath);
    let id;
    if (requestedId !== undefined) {
      if (document.features.some((feature) => feature.id === requestedId)) {
        throw new StoreError("conflict", `A feature with id "${requestedId}" already exists.`);
      }
      id = requestedId;
    } else {
      id = uniqueId(document.features, slugify(title));
    }
    const feature = {
      id,
      title,
      area,
      status,
      summary,
      owner,
      references,
      ...(image ? { image } : {}),
      updatedAt: new Date().toISOString(),
    };
    document.features.push(feature);
    const saved = await writeDocument(artifactPath, document);
    return { artifactPath, feature, summary: summarizeDocument(saved) };
  });
}

export async function updateFeature(workspaceRoot, input = {}) {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    invalid("Feature input must be an object.");
  }
  const id = textField(input.id, "id", { max: MAX_ID, required: true });
  const patch = {};
  if ("title" in input) patch.title = textField(input.title, "title", { max: MAX_TITLE, required: true });
  if ("area" in input) patch.area = textField(input.area, "area", { max: MAX_AREA }) || "General";
  if ("status" in input) patch.status = parseStatus(input.status);
  if ("summary" in input) patch.summary = textField(input.summary, "summary", { max: MAX_SUMMARY }) ?? "";
  if ("owner" in input) patch.owner = textField(input.owner, "owner", { max: MAX_OWNER }) ?? "";
  if ("references" in input) patch.references = referencesField(input.references) ?? [];
  if ("image" in input) patch.image = imageField(input.image);
  if (Object.keys(patch).length === 0) {
    invalid("Provide at least one of title, area, status, summary, owner, references or image.");
  }

  const artifactPath = artifactPathFor(workspaceRoot);
  return withDocumentLock(artifactPath, async () => {
    const { document, missing } = await readDocument(artifactPath);
    if (missing) throw new StoreError("not-found", `No feature with id "${id}"; ${artifactPath} does not exist yet.`);
    const feature = document.features.find((candidate) => candidate.id === id);
    if (!feature) throw new StoreError("not-found", `No feature with id "${id}".`);
    Object.assign(feature, patch, { updatedAt: new Date().toISOString() });
    if (!feature.image) delete feature.image;
    const saved = await writeDocument(artifactPath, document);
    return { artifactPath, feature, summary: summarizeDocument(saved) };
  });
}

export async function removeFeature(workspaceRoot, id) {
  const featureId = textField(id, "id", { max: MAX_ID, required: true });
  const artifactPath = artifactPathFor(workspaceRoot);
  return withDocumentLock(artifactPath, async () => {
    const { document, missing } = await readDocument(artifactPath);
    if (missing) throw new StoreError("not-found", `No feature with id "${featureId}"; ${artifactPath} does not exist yet.`);
    const index = document.features.findIndex((feature) => feature.id === featureId);
    if (index === -1) throw new StoreError("not-found", `No feature with id "${featureId}".`);
    const [removed] = document.features.splice(index, 1);
    const saved = await writeDocument(artifactPath, document);
    return { artifactPath, removed, summary: summarizeDocument(saved) };
  });
}

export async function summarizeFeatures(workspaceRoot) {
  const state = await readState(workspaceRoot);
  return {
    artifactPath: state.artifactPath,
    title: state.title,
    updatedAt: state.updatedAt,
    summary: state.summary,
  };
}
