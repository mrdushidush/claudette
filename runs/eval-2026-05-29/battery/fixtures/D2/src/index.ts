// Application entry point.
import { createSession, refreshToken } from './auth/session.ts';
import { greet } from './user.ts';

// We call refreshToken here, but it is NOT defined in this file.
const s = createSession(1, 'initial-token');
const refreshed = refreshToken(s);

console.log(greet('world'), refreshed.accessToken);
