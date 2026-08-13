// Split out from session.ts so middleware.ts (which runs on the Edge
// runtime) can import just the cookie name without pulling in
// next/headers, next/navigation, or the heavy-api fetch wrapper.
export const SESSION_COOKIE = "agentops_session";
