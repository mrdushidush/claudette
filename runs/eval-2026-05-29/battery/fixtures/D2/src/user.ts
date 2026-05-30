// User-facing helpers.

export function greet(name: string): string {
  return `Hello, ${name}!`;
}

// A decoy: this looks token-related but does not define refreshToken.
export function maskToken(token: string): string {
  return token.length <= 4 ? '****' : token.slice(0, 2) + '****';
}
