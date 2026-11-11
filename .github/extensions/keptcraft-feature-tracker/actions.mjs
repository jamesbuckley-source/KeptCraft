import { CanvasError } from "@github/copilot-sdk/extension";

import * as store from "./store.mjs";

const statusSchema = {
  type: "string",
  enum: [...store.STATUSES],
  description: "Lifecycle status of the feature.",
};

const featureFields = {
  title: { type: "string", description: "Feature title shown on the board." },
  area: { type: "string", description: "Grouping such as Capture, Search or Settings." },
  status: statusSchema,
  summary: { type: "string", description: "One or two sentences describing the feature." },
  owner: { type: "string", description: "Person or team responsible, when known." },
  references: {
    type: "array",
    items: { type: "string" },
    description: "File paths, commits or links related to the feature.",
  },
};

function invoke(fn) {
  return async (ctx) => {
    const workspaceRoot = store.resolveWorkspaceRoot(ctx);
    try {
      return await fn(workspaceRoot, ctx?.input ?? {});
    } catch (error) {
      if (error instanceof store.StoreError) {
        throw new CanvasError(`feature_tracker_${error.code.replace(/-/g, "_")}`, error.message);
      }
      throw error;
    }
  };
}

function compact(feature) {
  return {
    id: feature.id,
    title: feature.title,
    area: feature.area,
    status: feature.status,
    summary: feature.summary,
    owner: feature.owner || null,
    references: feature.references,
    updatedAt: feature.updatedAt,
  };
}

export const actions = [
  {
    name: "list_features",
    description: "List tracked KeptCraft features, optionally filtered by status, area or search text.",
    inputSchema: {
      type: "object",
      properties: {
        status: statusSchema,
        area: { type: "string", description: "Only features in this area." },
        query: { type: "string", description: "Case-insensitive text matched against id, title, area, summary and references." },
      },
      additionalProperties: false,
    },
    handler: invoke(async (workspaceRoot, input) => {
      const result = await store.listFeatures(workspaceRoot, input);
      return {
        artifactPath: result.artifactPath,
        missing: result.missing,
        updatedAt: result.updatedAt,
        matched: result.matched,
        summary: result.summary,
        features: result.features.map(compact),
      };
    }),
  },
  {
    name: "add_feature",
    description: "Add a feature to docs/features.json and return the new entry.",
    inputSchema: {
      type: "object",
      properties: {
        id: { type: "string", description: "Optional kebab-case id; derived from the title when omitted." },
        ...featureFields,
      },
      required: ["title"],
      additionalProperties: false,
    },
    handler: invoke(async (workspaceRoot, input) => {
      const result = await store.addFeature(workspaceRoot, input);
      return { artifactPath: result.artifactPath, feature: compact(result.feature), summary: result.summary };
    }),
  },
  {
    name: "update_feature",
    description: "Update fields or move a tracked feature to another status.",
    inputSchema: {
      type: "object",
      properties: {
        id: { type: "string", description: "Kebab-case id of the feature to update." },
        ...featureFields,
      },
      required: ["id"],
      additionalProperties: false,
    },
    handler: invoke(async (workspaceRoot, input) => {
      const result = await store.updateFeature(workspaceRoot, input);
      return { artifactPath: result.artifactPath, feature: compact(result.feature), summary: result.summary };
    }),
  },
  {
    name: "remove_feature",
    description: "Remove a feature from the tracker by id.",
    inputSchema: {
      type: "object",
      properties: {
        id: { type: "string", description: "Kebab-case id of the feature to remove." },
      },
      required: ["id"],
      additionalProperties: false,
    },
    handler: invoke(async (workspaceRoot, input) => {
      const result = await store.removeFeature(workspaceRoot, input.id);
      return { artifactPath: result.artifactPath, removed: compact(result.removed), summary: result.summary };
    }),
  },
  {
    name: "summarize_features",
    description: "Summarize how many tracked features are in the backlog, planned, in progress, blocked or shipped.",
    handler: invoke(async (workspaceRoot) => store.summarizeFeatures(workspaceRoot)),
  },
];
