import { Hono } from 'hono';
import users from './routes/users';
import posts from './routes/posts';
import { items } from './routes/items.js';
import admin from './routes/admin';

const app = new Hono();
const dynamic = 'x';

app.get('/health', (c) => c.json({ ok: true }));
app.all('/any', (c) => c.text('any'));
app.get(`/dyn/${dynamic}`, (c) => c.text('dyn'));

app.route('/users', users);
app.route('/posts', posts);
app.route('/items', items);
app.route('/internal', admin);

const api = new Hono();
api.get('/version', (c) => c.json({ v: 1 }));
app.route('/api', api);

export default app;
