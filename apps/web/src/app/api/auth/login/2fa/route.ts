import type { NextRequest } from "next/server";
import { proxyLogin2fa } from "@/lib/server/auth-proxy";

export async function POST(req: NextRequest) {
  return proxyLogin2fa(req);
}
