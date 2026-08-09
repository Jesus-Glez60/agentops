import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// RTL's auto-cleanup registers against Jest's global afterEach; under
// Vitest it needs to be wired up explicitly or every test's rendered DOM
// piles up across the file (confirmed: without this, a later test's
// `getByText` matched leftover elements from an earlier render).
afterEach(() => {
  cleanup();
});

// jsdom's own localStorage gets shadowed by Node's built-in experimental
// `localStorage` global in this exact Node/jsdom version combination
// (confirmed: without this, `window.localStorage` throws
// "localStorage is not available because --localstorage-file was not
// provided" at test time) -- a small deterministic in-memory shim avoids
// depending on that Node flag or any particular Node/jsdom version pairing.
class MemoryStorage implements Storage {
  private store = new Map<string, string>();

  get length() {
    return this.store.size;
  }

  clear(): void {
    this.store.clear();
  }

  getItem(key: string): string | null {
    return this.store.has(key) ? this.store.get(key)! : null;
  }

  key(index: number): string | null {
    return Array.from(this.store.keys())[index] ?? null;
  }

  removeItem(key: string): void {
    this.store.delete(key);
  }

  setItem(key: string, value: string): void {
    this.store.set(key, String(value));
  }
}

Object.defineProperty(window, "localStorage", {
  value: new MemoryStorage(),
  writable: true,
});
