import { createRoute } from "@hono/zod-openapi";

import createApp from "../lib/create-app";
import { exportStudentsRoute, managedStudentsRoutes } from "./admin-students";
import * as students from "./admin-students/students.routes";

const adminRouter = createApp();

// Middleware registered with `.on()`: every handler is a call expression and
// every path is also registered below, so it must not produce routes.
const adminChecker = (_role: string) => async (_c: unknown, next: () => Promise<void>) => next();
adminRouter.on(["POST", "PATCH"], ["/students", "/students/:studentId"], adminChecker("SUPER_ADMIN"));
adminRouter.use("/students/*", adminChecker("ADMIN"));

adminRouter.get("/stats", (c) => c.json({ users: c.req.query("window") }));

const localRoutes = {
  ping: createRoute({
    method: "get",
    path: "/ping",
    responses: { 200: { description: "Pong" } },
  }),
  exportStudentsRoute,
};

adminRouter
  .openapi(managedStudentsRoutes.list, (c) => c.json({}))
  .openapi(managedStudentsRoutes["get"], (c) => c.json({}))
  .openapi(managedStudentsRoutes.create, (c) => c.json({}, 201))
  .openapi(managedStudentsRoutes.update, (c) => c.json({}))
  .openapi(students.gradeRoutes.grades.list, (c) => c.json({}))
  .openapi(localRoutes.ping, (c) => c.json({}))
  .openapi(localRoutes.exportStudentsRoute, (c) => c.json({}));

export default adminRouter;
