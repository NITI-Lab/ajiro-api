import { NextResponse } from 'next/server';

type Ctx = { params: Promise<{ id: string }> };

export const GET = async (req: Request, ctx: Ctx) => NextResponse.json({});

export const PUT = async (req: Request, ctx: Ctx) => NextResponse.json({});

export async function DELETE(req: Request, ctx: Ctx) {
  return new Response(null, { status: 204 });
}

export const dynamic = 'force-dynamic';
