import { OpenAPIHono } from "@hono/zod-openapi";

import type { AppBindings } from "./types";

export function createRouter() {
  return new OpenAPIHono<AppBindings>({
    strict: false,
    defaultHook: (result, c) => {
      if (!result.success) {
        return c.json({ ok: false }, 422);
      }
    },
  });
}

export default function createApp() {
  const app = createRouter();
  app.use("*", async (c, next) => {
    await next();
  });
  app.notFound((c) => c.json({ message: "nope" }, 404));
  return app;
}
