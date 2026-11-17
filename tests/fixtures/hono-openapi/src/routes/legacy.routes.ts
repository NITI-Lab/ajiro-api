import { createRoute } from "@hono/zod-openapi";

// Defined but never registered through `.openapi()`.
export const ping = createRoute({
  method: "get",
  path: "/legacy/ping",
  responses: { 200: { description: "Pong" } },
});
