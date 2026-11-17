import { createRoute } from "@hono/zod-openapi";

import { API_VERSION } from "../../lib/constants";

export const getReport = createRoute({
  method: "get",
  path: `${API_VERSION}/reports/{id}`,
  responses: { 200: { description: "Report" } },
});

// Not derivable statically: skipped with a warning.
export const dynamicReport = createRoute({
  method: "get",
  path: `${process.env.PREFIX}/reports/dynamic`,
  responses: { 200: { description: "Dynamic" } },
});
