import { Hono } from "hono";
import { requireAuth } from "../auth.js";
import { createTodo, deleteTodo, findByTitle, getTodo, listTodos, todoStats, updateTodo } from "../store.js";

export const todoRoutes = new Hono();

todoRoutes.use("*", requireAuth);

function validateTitle(value: unknown): string | null {
  if (typeof value !== "string") return "title must be a string";
  const title = value.trim();
  if (title.length === 0) return "title must not be empty";
  if (title.length > 100) return "title must be 100 characters or fewer";
  return null;
}

todoRoutes.get("/", (c) => {
  const done = c.req.query("done");
  if (done !== undefined && done !== "true" && done !== "false") {
    return c.json({ error: "validation_error", message: "done must be true or false" }, 400);
  }
  return c.json({ items: listTodos(done === undefined ? undefined : done === "true") });
});

// Must be registered before "/:id" so "stats" is not treated as an id.
todoRoutes.get("/stats", (c) => c.json(todoStats()));

todoRoutes.post("/", async (c) => {
  const body = await c.req.json().catch(() => null);
  const problem = validateTitle(body?.title);
  if (problem) {
    return c.json({ error: "validation_error", message: problem }, 400);
  }
  if (findByTitle(body.title)) {
    return c.json({ error: "duplicate_title", message: "A todo with this title already exists" }, 409);
  }
  return c.json(createTodo(body.title), 201);
});

todoRoutes.get("/:id", (c) => {
  const todo = getTodo(c.req.param("id"));
  if (!todo) {
    return c.json({ error: "not_found", message: "Todo not found" }, 404);
  }
  return c.json(todo);
});

todoRoutes.patch("/:id", async (c) => {
  const id = c.req.param("id");
  const body = await c.req.json().catch(() => null);
  if (!body || typeof body !== "object") {
    return c.json({ error: "validation_error", message: "JSON body is required" }, 400);
  }
  const patch: { title?: string; done?: boolean } = {};
  if ("title" in body) {
    const problem = validateTitle(body.title);
    if (problem) return c.json({ error: "validation_error", message: problem }, 400);
    patch.title = body.title.trim();
  }
  if ("done" in body) {
    if (typeof body.done !== "boolean") {
      return c.json({ error: "validation_error", message: "done must be a boolean" }, 400);
    }
    patch.done = body.done;
  }
  const current = getTodo(id);
  if (!current) {
    return c.json({ error: "not_found", message: "Todo not found" }, 404);
  }
  if (patch.title) {
    const other = findByTitle(patch.title);
    if (other && other.id !== id) {
      return c.json({ error: "duplicate_title", message: "A todo with this title already exists" }, 409);
    }
  }
  return c.json(updateTodo(id, patch));
});

todoRoutes.delete("/:id", (c) => {
  if (!deleteTodo(c.req.param("id"))) {
    return c.json({ error: "not_found", message: "Todo not found" }, 404);
  }
  return c.body(null, 204);
});
