import { createRoute } from "@hono/zod-openapi";

const BASE = "/v1";
const TASKS = `${BASE}/tasks`;

export const list = createRoute({
  method: "get",
  path: `${BASE}/tasks`,
  responses: { 200: { description: "List" } },
});

export const patch = createRoute({
  method: "patch",
  path: `${TASKS}/{id}`,
  responses: { 200: { description: "Patched" } },
});
