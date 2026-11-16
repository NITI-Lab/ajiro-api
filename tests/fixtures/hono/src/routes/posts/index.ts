import { Hono } from 'hono';

const posts = new Hono();

posts.on(['GET', 'POST'], '/', (c) => c.json([]));
posts.on('DELETE', '/:postId', (c) => c.body(null, 204));
posts.get('/:postId/comments', (c) => c.json([]));

export default posts;
