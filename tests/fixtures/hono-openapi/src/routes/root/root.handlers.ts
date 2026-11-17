import type { AppRouteHandler } from "@/lib/types";

import type { IndexRoute } from "./root.routes";

export const index: AppRouteHandler<IndexRoute> = (c) => c.json({ hello: "world" });
