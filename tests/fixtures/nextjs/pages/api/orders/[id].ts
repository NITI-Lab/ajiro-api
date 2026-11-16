import type { NextApiRequest, NextApiResponse } from 'next';

export default function orderHandler(req: NextApiRequest, res: NextApiResponse) {
  if (req.method === 'DELETE') {
    return res.status(204).end();
  }
  if (req.method !== 'GET') {
    return res.status(405).end();
  }
  res.json({});
}
