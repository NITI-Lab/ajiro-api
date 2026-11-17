export const list = (c: any) => c.json([]);
export const getOne = (c: any) => c.json({});
export const create = (c: any) => c.json({}, 201);
export const remove = (c: any) => c.body(null, 204);
