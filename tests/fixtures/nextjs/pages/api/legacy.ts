export default async function handler(req, res) {
  switch (req.method) {
    case 'POST':
      return res.status(201).json({});
    case 'GET':
      return res.json([]);
    default:
      return res.status(405).end();
  }
}
