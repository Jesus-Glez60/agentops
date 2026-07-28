import { formatName, Cache } from './util';

const cache = new Cache();

export function greet(first: string, last: string): string {
  const name = formatName(first, last);
  cache.set('lastGreeted', name);
  return `Hello, ${name}!`;
}
