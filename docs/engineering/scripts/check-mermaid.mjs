// Parses every ```mermaid block in the given Markdown files; exits 1 on any parse error.
// ES-module imports resolve next to this file, so run a copy placed beside `mermaid` and `jsdom`:
//   D=$(mktemp -d) && (cd $D && npm init -y >/dev/null && npm i mermaid@11 jsdom@24)
//   cp docs/engineering/scripts/check-mermaid.mjs $D/ && node $D/check-mermaid.mjs $(grep -rl "\`\`\`mermaid" docs/engineering)
import { JSDOM } from 'jsdom';
import fs from 'fs';
const dom = new JSDOM('<!doctype html><html><body></body></html>');
globalThis.window = dom.window; globalThis.document = dom.window.document;
globalThis.DOMParser = dom.window.DOMParser; globalThis.Element = dom.window.Element;
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
const { default: mermaid } = await import('mermaid');
mermaid.initialize({ startOnLoad: false });
let bad = 0, n = 0;
for (const f of process.argv.slice(2)) {
  const md = fs.readFileSync(f, 'utf8');
  const blocks = [...md.matchAll(/```mermaid\n([\s\S]*?)```/g)].map(m => m[1]);
  for (const [i, b] of blocks.entries()) {
    n++;
    try { await mermaid.parse(b); console.log('ok  ', f, '#' + i); }
    catch (e) { bad++; console.log('FAIL', f, '#' + i, String(e.message || e).split('\n').slice(0, 4).join(' | ')); }
  }
}
console.log(`${n} diagrams, ${bad} failed`); process.exit(bad ? 1 : 0);
