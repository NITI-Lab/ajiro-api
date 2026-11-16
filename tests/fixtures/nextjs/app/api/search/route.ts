import { type NextRequest } from 'next/server';

export async function GET(request: NextRequest) {
  const q = request.nextUrl.searchParams.get('q');
  const page = new URL(request.url).searchParams.get('page');
  return Response.json({ q, page });
}
