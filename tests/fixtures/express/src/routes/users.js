const express = require('express');
const router = express.Router();

router.get('/', (req, res) => {
  const { page, limit } = req.query;
  res.json({ page, limit });
});
router.get('/:id', (req, res) => res.json({}));
router.post('/', (req, res) => res.status(201).json({}));

module.exports = router;
