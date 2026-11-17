import { createRouter } from "@/lib/create-app";

import { list, patch } from "./tasks.routes";

const router = createRouter();

router.openapi(list, (c) => c.json([c.req.query("status")]));
router.openapi(patch, (c) => c.json({}));

export default router;
