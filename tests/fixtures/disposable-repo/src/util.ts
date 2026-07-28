export function formatName(first: string, last: string): string {
  return `${first} ${last}`;
}

export class Cache {
  private store: Record<string, unknown> = {};

  get(key: string) {
    return this.store[key];
  }

  set(key: string, value: unknown) {
    this.store[key] = value;
  }
}
