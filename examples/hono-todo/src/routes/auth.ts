import { Hono } from "hono";
import { DEMO_PASSWORD, DEMO_USERNAME, issueToken } from "../auth.js";

export const authRoutes = new Hono();

authRoutes.post("/login", async (c) => {
  const body = await c.req.json().catch(() => null);
  if (!body || typeof body.username !== "string" || typeof body.password !== "string") {
    return c.json({ error: "validation_error", message: "username and password are required" }, 400);
  }
  if (body.username !== DEMO_USERNAME || body.password !== DEMO_PASSWORD) {
    return c.json({ error: "invalid_credentials", message: "Wrong username or password" }, 401);
  }
  return c.json({ token: issueToken() });
});
