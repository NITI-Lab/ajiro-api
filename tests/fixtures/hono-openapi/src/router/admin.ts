import createApp from "../lib/create-app";

const adminRouter = createApp();

adminRouter.get("/stats", (c) => c.json({ users: c.req.query("window") }));

export default adminRouter;
