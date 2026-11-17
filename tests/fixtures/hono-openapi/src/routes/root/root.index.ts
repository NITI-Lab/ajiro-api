import { createRouter } from "@/lib/create-app";

import * as handlers from "./root.handlers";
import * as routes from "./root.routes";

const router = createRouter().openapi(routes.index, handlers.index);

export default router;
