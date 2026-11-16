import { randomUUID } from "node:crypto";
import type { MiddlewareHandler } from "hono";

export const DEMO_USERNAME = "demo";
export const DEMO_PASSWORD = process.env.DEMO_PASSWORD ?? "demo-password";

const tokens = new Set<string>();

export function issueToken(): string {
  const token = `tok_${randomUUID()}`;
  tokens.add(token);
  return token;
}

/** Requires `Authorization: Bearer <token>` issued by POST /auth/login. */
export const requireAuth: MiddlewareHandler = async (c, next) => {
  const header = c.req.header("Authorization") ?? "";
  const token = header.startsWith("Bearer ") ? header.slice("Bearer ".length) : "";
  if (!token || !tokens.has(token)) {
    return c.json({ error: "unauthorized", message: "Missing or invalid bearer token" }, 401);
  }
  await next();
};
