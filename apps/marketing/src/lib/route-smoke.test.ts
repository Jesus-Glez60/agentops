// Route/link smoke test over the actual `dist/` build output -- the same
// class of check apps/web's proxy.test.ts / base-path.test.ts exist for
// (see their doc comments): "passes build/tsc/tests but 404s live" is a
// recorded recurring failure mode for this repo's cross-app routing, and
// this app introduces the identical shape of risk (header/footer links
// resolving only once real routing exists). Requires `npm run build` to
// have run first (see package.json's `pretest` script).
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const DIST = join(__dirname, '../../dist');

function readAllHtml(dir: string): { path: string; html: string }[] {
  const out: { path: string; html: string }[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...readAllHtml(full));
    else if (entry.name.endsWith('.html')) out.push({ path: full, html: readFileSync(full, 'utf-8') });
  }
  return out;
}

describe('built site routes', () => {
  it('dist/ exists (run `npm run build` first)', () => {
    expect(existsSync(DIST)).toBe(true);
  });

  it('every internal /features/* and /pricing/ href resolves to a real generated page', () => {
    const pages = readAllHtml(DIST);
    expect(pages.length).toBeGreaterThan(0);

    const internalHrefPattern = /href="(\/(?:features\/[a-z-]+|pricing)\/)"/g;
    const missing: string[] = [];
    for (const { html } of pages) {
      for (const match of html.matchAll(internalHrefPattern)) {
        const href = match[1];
        const expected = join(DIST, href, 'index.html');
        if (!existsSync(expected)) missing.push(href);
      }
    }
    expect([...new Set(missing)]).toEqual([]);
  });

  it('no page links to a login/register flow -- this site has no auth infra to back one', () => {
    // This app deliberately never offers login/register (no backing infra
    // for it) -- every former "Login / Register" spot now points at the
    // README's self-host quick start instead (see src/lib/external-links.ts).
    // This guards against that CTA quietly regressing back to a /suite/login
    // link, which is exactly the class of bug this repo hit in production.
    //
    // Checks every generated page (Header/Footer render on all of them, not
    // just the homepage) and only the literal href attribute value -- not
    // the word "login" anywhere on the page -- since the Team & Settings
    // feature page legitimately describes the *product's own* login route
    // in prose ("The first visitor to /login sets up the org...").
    const pages = readAllHtml(DIST);
    expect(pages.length).toBeGreaterThan(0);
    const hrefPattern = /href="([^"]*)"/g;
    for (const { path, html } of pages) {
      for (const match of html.matchAll(hrefPattern)) {
        expect(match[1].toLowerCase(), `${path} links to a login route: ${match[1]}`).not.toContain('login');
      }
      expect(html, `${path} missing the self-host deploy link`).toContain('href="https://github.com/Jesus-Glez60/agentops#quick-start"');
    }
  });

  it('the Documentation link points at the external hosted docs, not a local /docs/ route', () => {
    const home = readFileSync(join(DIST, 'index.html'), 'utf-8');
    expect(home).toContain('href="https://docs.agentops.dedyn.io/"');
    expect(home).not.toContain('href="/docs/"');
  });

  it('llms.txt and llms-full.txt are served from the site root', () => {
    expect(existsSync(join(DIST, 'llms.txt'))).toBe(true);
    expect(existsSync(join(DIST, 'llms-full.txt'))).toBe(true);
  });

  it('every page has a canonical link and a non-empty description', () => {
    const pages = readAllHtml(DIST);
    for (const { path, html } of pages) {
      expect(html, `${path} missing canonical link`).toMatch(/<link rel="canonical" href="[^"]+"/);
      expect(html, `${path} missing og:description`).toMatch(/<meta property="og:description" content="[^"]+"/);
    }
  });

  it("pricing's description differs from the homepage's default -- regression guard for a real bug this caught", () => {
    // pricing.astro once omitted the `description` prop entirely, so its
    // meta/og/twitter description silently fell back to Layout.astro's
    // generic homepage tagline -- caught by a 5-persona diff review, not by
    // any test, which is why this exists now.
    const home = readFileSync(join(DIST, 'index.html'), 'utf-8');
    const pricing = readFileSync(join(DIST, 'pricing', 'index.html'), 'utf-8');
    const homeDesc = home.match(/<meta name="description" content="([^"]+)"/)?.[1];
    const pricingDesc = pricing.match(/<meta name="description" content="([^"]+)"/)?.[1];
    expect(homeDesc).toBeTruthy();
    expect(pricingDesc).toBeTruthy();
    expect(pricingDesc).not.toBe(homeDesc);
  });
});
