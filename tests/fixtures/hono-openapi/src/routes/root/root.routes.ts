import { createRoute } from "@hono/zod-openapi";

export const index = createRoute({
  method: "get",
  path: "/",
  responses: { 200: { description: "Index" } },
});

export type IndexRoute = typeof index;
