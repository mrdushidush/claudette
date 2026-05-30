import type { User } from './types.ts';

export function label(u: User): string {
  return u.name;
}
