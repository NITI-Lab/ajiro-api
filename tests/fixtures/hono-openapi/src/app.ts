import configureOpenAPI from "@/lib/configure-open-api";
import createApp from "@/lib/create-app";
import adminRouter from "@/router/admin";
import health from "@/router/index";
import reports from "@/routes/reports";
import root from "@/routes/root/root.index";
import tasks from "@/routes/tasks/tasks.index";
import users from "@/routes/users/users.index";

const app = createApp().basePath("/api");

configureOpenAPI(app);

const routes = [root, users, tasks, reports, health] as const;

routes.forEach((route) => {
  app.route("/", route);
});

app.route("/admin", adminRouter);

export type AppType = (typeof routes)[number];

export default app;
