import { NextResponse } from 'next/server';

export async function GET() {
  return NextResponse.json([]);
}

export function POST(req: Request) {
  return NextResponse.json({}, { status: 201 });
}
