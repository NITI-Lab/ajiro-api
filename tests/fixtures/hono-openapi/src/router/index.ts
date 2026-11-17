import { createRouter } from "@/lib/create-app";

const health = createRouter();

health.get("/health", (c) => c.json({ ok: true }));

export default health;
