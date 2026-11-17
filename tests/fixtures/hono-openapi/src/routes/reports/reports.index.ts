import { createRoute } from "@hono/zod-openapi";

import { createRouter } from "@/lib/create-app";

import { API_VERSION } from "../../lib/constants";
import * as routes from "./reports.routes";

const router = createRouter()
  .openapi(routes.getReport, (c) => c.json({}))
  .openapi(
    createRoute({
      method: "post",
      path: API_VERSION + "/reports",
      responses: { 202: { description: "Accepted" } },
    }),
    (c) => c.json({}, 202),
  );

export default router;
