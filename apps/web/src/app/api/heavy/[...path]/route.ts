import type { NextRequest } from "next/server";
import { proxyHeavyApi } from "@/lib/server/heavy-proxy";

async function handle(req: NextRequest, { params }: { params: Promise<{ path: string[] }> }) {
  const { path } = await params;
  return proxyHeavyApi(req, path);
}

export { handle as GET, handle as POST, handle as PATCH, handle as PUT, handle as DELETE };
