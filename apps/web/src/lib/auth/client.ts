// Client-side auth calls -- always through this app's own /api/auth/* BFF
// routes, never straight to agentops-heavy-api (see heavy-api.ts's doc
// comment for why). The login/signup forms call these two functions
// instead of inlining fetch, so a future "or continue with…" method can be
// added here without restructuring the forms themselves.
import type { SessionUser } from "@/lib/auth/types";

export class AuthClientError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AuthClientError";
  }
}

async function postAuth(path: string, body: Record<string, string>): Promise<SessionUser> {
  const res = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await res.json().catch(() => null);

  if (!res.ok) {
    const message = data && typeof data === "object" && typeof data.error === "string" ? data.error : "Something went wrong. Please try again.";
    throw new AuthClientError(message);
  }

  return data.user as SessionUser;
}

export function loginWithPassword(email: string, password: string): Promise<SessionUser> {
  return postAuth("/api/auth/login", { email, password });
}

export function signupWithPassword(firstName: string, lastName: string, email: string, password: string): Promise<SessionUser> {
  return postAuth("/api/auth/signup", { first_name: firstName, last_name: lastName, email, password });
}

export async function logout(): Promise<void> {
  await fetch("/api/auth/logout", { method: "POST" });
}
