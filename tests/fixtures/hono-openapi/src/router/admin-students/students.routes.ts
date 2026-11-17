import { createRoute } from "@hono/zod-openapi";

export const listStudentsRoute = createRoute({
  method: "get",
  path: "/students",
  responses: { 200: { description: "Students" } },
});

export const getStudentRoute = createRoute({
  method: "get",
  path: "/students/{studentId}",
  responses: { 200: { description: "Student" } },
});

export const exportStudentsRoute = createRoute({
  method: "get",
  path: "/students/export",
  responses: { 200: { description: "CSV" } },
});

// Routes grouped in an object literal: referenced as `managedStudentsRoutes.list`.
export const managedStudentsRoutes = {
  list: listStudentsRoute,
  get: getStudentRoute,
  create: createRoute({
    method: "post",
    path: "/students",
    responses: { 201: { description: "Created" } },
  }),
  update: createRoute({
    method: "patch",
    path: "/students/{studentId}",
    responses: { 200: { description: "Updated" } },
  }),
} as const;

export const gradeRoutes = {
  grades: {
    list: createRoute({
      method: "get",
      path: "/grades",
      responses: { 200: { description: "Grades" } },
    }),
  },
} satisfies Record<string, unknown>;
