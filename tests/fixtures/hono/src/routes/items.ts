import { OpenAPIHono, createRoute } from '@hono/zod-openapi';

export const items = new OpenAPIHono();

const getItem = createRoute({
  method: 'get',
  path: '/{id}',
  responses: { 200: { description: 'ok' } },
});

items.openapi(getItem, (c) => c.json({}));
items.get('/', (c) => c.json([]));
