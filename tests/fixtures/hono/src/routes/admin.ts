import { Hono } from 'hono';

const admin = new Hono().basePath('/admin');

admin.get('/stats', (c) => c.json({}));

export default admin;
