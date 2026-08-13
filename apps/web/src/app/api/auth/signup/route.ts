import type { NextRequest } from "next/server";
import { proxyCredentialsAuth } from "@/lib/server/auth-proxy";

export async function POST(req: NextRequest) {
  return proxyCredentialsAuth(req, "/auth/signup", 201);
}
