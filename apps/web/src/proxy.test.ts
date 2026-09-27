import { describe, expect, it } from "vitest";
import { NextRequest } from "next/server";
import { SESSION_COOKIE } from "@/lib/auth/constants";
import { BASE_PATH } from "@/lib/base-path";
import { proxy } from "@/proxy";

// Next.js strips `basePath` from `request.nextUrl` before middleware ever
// runs (see proxy.ts's own comment) -- so a request built here must NOT
// include BASE_PATH either, matching what proxy() actually receives live.
function requestTo(pathAndQuery: string, cookie?: string) {
  const headers = cookie ? { cookie: `${SESSION_COOKIE}=${cookie}` } : undefined;
  return new NextRequest(`http://localhost${pathAndQuery}`, { headers });
}

describe("proxy", () => {
  it("passes through when the session cookie is present", () => {
    const response = proxy(requestTo("/repositories", "raw-session-token"));
    expect(response.status).toBe(200);
    expect(response.headers.get("location")).toBeNull();
  });

  // Regression test for the basePath gotcha recorded in this project's own
  // notes (proxy-ts-matcher-must-allowlist-every-pre-auth-route.md /
  // next-js-basepath-doesn-t-cover-raw-fetch-calls...md): `request.nextUrl`
  // has basePath already stripped, and NextResponse.redirect(new URL(...))
  // does NOT re-add it, so both the redirect target and the `from` param
  // must be built with BASE_PATH by hand or this silently 404s live while
  // still passing build/tsc/tests.
  it("redirects to the BASE_PATH-prefixed login page when there's no session cookie", () => {
    const response = proxy(requestTo("/repositories?tab=connect"));

    expect(response.status).toBe(307);
    const location = new URL(response.headers.get("location")!);
    expect(location.pathname).toBe(`${BASE_PATH}/login`);
    expect(location.searchParams.get("from")).toBe(`${BASE_PATH}/repositories?tab=connect`);
  });

  it("redirects to the BASE_PATH-prefixed login page for the site root", () => {
    const response = proxy(requestTo("/"));

    const location = new URL(response.headers.get("location")!);
    expect(location.pathname).toBe(`${BASE_PATH}/login`);
    expect(location.searchParams.get("from")).toBe(`${BASE_PATH}/`);
  });
});
