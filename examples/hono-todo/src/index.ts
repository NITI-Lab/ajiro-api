import { serve } from "@hono/node-server";
import { Hono } from "hono";
import { authRoutes } from "./routes/auth.js";
import { todoRoutes } from "./routes/todos.js";

const app = new Hono();

app.get("/health", (c) => c.json({ status: "ok" }));

app.route("/auth", authRoutes);
app.route("/todos", todoRoutes);

app.notFound((c) => c.json({ error: "not_found", message: "Route not found" }, 404));

const port = Number(process.env.PORT ?? 3000);
serve({ fetch: app.fetch, port }, (info) => {
  console.log(`hono-todo listening on http://localhost:${info.port}`);
});
