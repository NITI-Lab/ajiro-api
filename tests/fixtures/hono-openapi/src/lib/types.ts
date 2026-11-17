import type { OpenAPIHono } from "@hono/zod-openapi";

export interface AppBindings {
  Variables: { requestId: string };
}

export type AppOpenAPI = OpenAPIHono<AppBindings>;
