import { Hono } from 'hono';
import { listUsers, createUser } from '../handlers';

export const users = new Hono();

users.get('/', listUsers);
users.post('/', createUser);
users
  .get('/:id', (c) => c.json({ id: c.req.param('id') }))
  .put('/:id{[0-9]+}', (c) => c.json({}))
  .delete('/:id', (c) => c.body(null, 204));

users.get('/search', (c) => {
  const name = c.req.query('name');
  const limit = c.req.query('limit');
  return c.json([name, limit]);
});

export default users;
