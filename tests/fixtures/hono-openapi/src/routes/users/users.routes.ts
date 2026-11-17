import { createRoute, z } from "@hono/zod-openapi";

const tags = ["Users"];

export const list = createRoute({
  path: "/users",
  method: "get",
  tags,
  responses: { 200: { description: "List" } },
});

export const getOne = createRoute({
  path: "/users/{id}",
  method: "get",
  tags,
  request: { params: z.object({ id: z.string() }) },
  responses: { 200: { description: "One" } },
});

export const create = createRoute({
  path: "/users",
  method: "post",
  tags,
  responses: { 201: { description: "Created" } },
});

export const remove = createRoute({
  path: "/users/{id}",
  method: "delete",
  tags,
  responses: { 204: { description: "Gone" } },
});

export type ListRoute = typeof list;
export type GetOneRoute = typeof getOne;
export type CreateRoute = typeof create;
export type RemoveRoute = typeof remove;
