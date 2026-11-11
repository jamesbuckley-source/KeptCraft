import { CanvasError, createCanvas, joinSession } from "@github/copilot-sdk/extension";

import { actions } from "./actions.mjs";
import { startInstanceServer } from "./server.mjs";
import { STATUSES, readState, resolveWorkspaceRoot } from "./store.mjs";

const CANVAS_ID = "keptcraft-feature-tracker";
const instances = new Map();

function normalizeView(input) {
  const view = {};
  if (input === undefined || input === null) return view;
  if (typeof input !== "object" || Array.isArray(input)) {
    throw new CanvasError("feature_tracker_invalid_input", "Open input must be an object.");
  }
  if (input.status !== undefined) {
    if (typeof input.status !== "string" || !STATUSES.includes(input.status)) {
      throw new CanvasError("feature_tracker_invalid_input", `status must be one of: ${STATUSES.join(", ")}.`);
    }
    view.status = input.status;
  }
  if (input.query !== undefined) {
    if (typeof input.query !== "string" || input.query.length > 200) {
      throw new CanvasError("feature_tracker_invalid_input", "query must be a string of at most 200 characters.");
    }
    view.query = input.query;
  }
  return view;
}

const canvas = createCanvas({
  id: CANVAS_ID,
  displayName: "KeptCraft Feature Tracker",
  description:
    "Track which KeptCraft features are in the backlog, planned, in progress, blocked or shipped in the versioned docs/features.json artifact.",
  inputSchema: {
    type: "object",
    properties: {
      status: {
        type: "string",
        enum: [...STATUSES],
        description: "Open the board filtered to this status.",
      },
      query: {
        type: "string",
        description: "Initial search text.",
      },
    },
    additionalProperties: false,
  },
  actions,
  async open(ctx) {
    const workspaceRoot = resolveWorkspaceRoot(ctx);
    const view = normalizeView(ctx.input);

    let instance = instances.get(ctx.instanceId);
    if (instance && instance.workspaceRoot !== workspaceRoot) {
      instances.delete(ctx.instanceId);
      await instance.server.close();
      instance = undefined;
    }
    if (instance) {
      instance.server.setView(view);
      return { url: instance.server.url, title: "KeptCraft Feature Tracker", status: await describeState(workspaceRoot) };
    }

    const server = await startInstanceServer({ workspaceRoot, view });
    instances.set(ctx.instanceId, { server, workspaceRoot });
    return { url: server.url, title: "KeptCraft Feature Tracker", status: await describeState(workspaceRoot) };
  },
  async onClose(ctx) {
    const instance = instances.get(ctx.instanceId);
    if (!instance) return;
    instances.delete(ctx.instanceId);
    await instance.server.close();
  },
});

async function describeState(workspaceRoot) {
  try {
    const state = await readState(workspaceRoot);
    return `${state.summary.total} features · ${state.summary.counts.shipped} shipped`;
  } catch {
    return "docs/features.json needs attention";
  }
}

const session = await joinSession({ canvases: [canvas] });

session
  .log(`KeptCraft feature tracker canvas ready (${CANVAS_ID}); board lives in docs/features.json.`)
  .catch(() => {});
