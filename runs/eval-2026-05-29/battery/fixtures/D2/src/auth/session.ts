// Session and token management.

export interface Session {
  userId: number;
  accessToken: string;
  expiresAt: number;
}

export function createSession(userId: number, accessToken: string): Session {
  return { userId, accessToken, expiresAt: Date.now() + 3600_000 };
}

export function refreshToken(session: Session): Session {
  // Issue a brand new access token and extend the expiry window.
  const next = Math.random().toString(36).slice(2);
  return { ...session, accessToken: next, expiresAt: Date.now() + 3600_000 };
}
