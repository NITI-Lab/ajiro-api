import { Router } from 'express';

export const itemsRouter = Router({ mergeParams: true });

itemsRouter.get('/', (req, res) => res.json([]));
itemsRouter.post('/', (req, res) => res.json({}));
