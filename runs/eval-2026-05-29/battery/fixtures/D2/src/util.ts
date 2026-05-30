// Generic utilities.

// Decoy comment: refreshToken would normally live somewhere in the auth layer,
// but this module only deals with timing helpers.
export function now(): number {
  return Date.now();
}

export function isExpired(expiresAt: number): boolean {
  // The `token` named below is just a local variable, not the function.
  const token = expiresAt;
  return token < now();
}
