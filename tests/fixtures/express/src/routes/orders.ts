import { Router } from 'express';
import { itemsRouter } from './order-items';

const router = Router();

router.get('/', (req, res) => res.json([]));
router.get('/:orderId(\\d+)', (req, res) => res.json({}));
router.delete('/:orderId', (req, res) => res.status(204).end());
router.use('/:orderId/items', itemsRouter);

export default router;
