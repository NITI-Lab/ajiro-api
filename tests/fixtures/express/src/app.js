const express = require('express');
const orders = require('./routes/orders');
const ctrl = require('./controllers/items');

const app = express();
app.use(express.json());

app.get('/health', (req, res) => res.json({ ok: true }));
app.get('env');

app.use('/users', require('./routes/users'));
app.use('/api/orders', orders);

app.route('/items')
  .get(ctrl.list)
  .post(ctrl.create);

app.get(/^\/regex$/, (req, res) => res.end());

module.exports = app;
