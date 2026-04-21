# Google OAuth setup for Claudette

Claudette talks to Google Calendar (and, later, Gmail) using your own OAuth
client. You create a tiny OAuth client in Google Cloud Console once, paste
the `client_id` and `client_secret` into Claudette's secrets dir, then run
`claudette --auth-google` to grant consent in your browser. Tokens are stored
locally at `~/.claudette/secrets/google_oauth.json` and refreshed
automatically; no Claudette server ever sees your data.

This is the standard "installed app" (desktop) OAuth flow. Google does not
charge for this; you're using your own quota.

## 1. Create a Google Cloud project

1. Open <https://console.cloud.google.com/>.
2. Top bar → project dropdown → **New Project**. Name: anything, e.g. `claudette-personal`.
3. Wait for the creation to finish, then make sure the new project is selected
   in the top bar.

## 2. Enable the APIs you want

Navigation → **APIs & Services → Library**. Search and enable:

- **Google Calendar API** — required for the `calendar` tool group.
- **Gmail API** — required for the `gmail` tool group (phase 4 onward).
  Read-only access only in v0.2.0; compose/send arrives in a later release.

## 3. Configure the OAuth consent screen

Navigation → **APIs & Services → OAuth consent screen**.

1. **User Type**: choose **External**. (Internal requires a Workspace org.)
2. Fill in the required fields:
   - App name: `Claudette` (or whatever you like — only you will see it)
   - User support email: your email
   - Developer contact email: your email
3. **Scopes**: leave empty for now. Claudette requests scopes dynamically.
4. **Test users**: add the Google account you want Claudette to act on
   behalf of (probably your own). This is essential — in Testing mode only
   listed test users can complete the flow.
5. **Publishing status**: leave as **Testing**. This avoids Google's app
   verification process entirely. Testing mode supports up to 100 test
   users, which is fine for a self-hosted single-user tool.

## 4. Create the OAuth client

Navigation → **APIs & Services → Credentials → Create Credentials → OAuth client ID**.

- **Application type**: **Desktop app**. (Do NOT pick "Web application" —
  that requires a fixed redirect URI registered in the console, while
  Claudette's loopback server picks a random free port each run.)
- **Name**: `Claudette local` (arbitrary).

Click **Create**. Google shows a dialog with the `client_id` and `client_secret`.
Keep this open — you need both strings in the next step.

## 5. Give Claudette the client credentials

Pick ONE of these. Env vars take precedence if both are set.

### Option A — environment variables (recommended for shell users)

```sh
export CLAUDETTE_GOOGLE_CLIENT_ID="1234567890-abc...apps.googleusercontent.com"
export CLAUDETTE_GOOGLE_CLIENT_SECRET="GOCSPX-..."
```

Add them to `~/.claudette/.env` (Claudette reads this file at startup) so
they persist across shells. The file format is a plain `KEY=VALUE` per line.

Short-form names (`GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`) also work, but
the `CLAUDETTE_` prefix avoids collisions with other tools.

### Option B — JSON file

Write `~/.claudette/secrets/google_oauth_client.json`:

```json
{
  "client_id": "1234567890-abc...apps.googleusercontent.com",
  "client_secret": "GOCSPX-..."
}
```

On Unix, Claudette will `chmod 0600` its own token files; you should do the
same for this one: `chmod 0600 ~/.claudette/secrets/google_oauth_client.json`.

## 6. Run the auth flow

You run the flow **once per scope bundle**. Claudette keeps Calendar and
Gmail tokens in separate files (AD-6: a hostile email read can't pivot
to Calendar writes), so you authorise each one individually.

**Calendar** (required for the `calendar` tool group and the morning briefing):

```sh
claudette --auth-google           # scope defaults to calendar
# or, explicitly:
claudette --auth-google calendar
```

**Gmail read-only** (required for the `gmail` tool group and the briefing's
email summary):

```sh
claudette --auth-google gmail
```

Either command:

1. Binds a loopback HTTP server on `127.0.0.1:<random-port>`.
2. Opens your default browser to Google's consent screen for that scope.
3. You sign in and approve the scopes.
4. Google redirects to `http://127.0.0.1:<port>/callback?code=...`.
5. Claudette captures the code, exchanges it for an access + refresh token,
   writes the context-specific file under `~/.claudette/secrets/`
   (`google_oauth.json` for calendar, `google_oauth_gmail_read.json` for
   gmail), and closes the server.

If the browser doesn't open automatically, copy the URL Claudette prints and
paste it into your browser yourself — the flow still completes.

Refresh tokens stay valid until you revoke them. You only rerun
`--auth-google <scope>` if Google invalidates the token (rare) or you
change which Google account the scope is bound to.

## 7. Verify it works

Inside Claudette (REPL or Telegram):

> what's on my calendar this week?

Claudette should call `enable_tools(calendar)` then `calendar_list_events`
and respond with your real events. If you get `not authenticated`, the
token file is missing — rerun `claudette --auth-google`.

## Revoking access

```sh
claudette --auth-google --revoke            # calendar (default)
claudette --auth-google calendar --revoke   # explicit
claudette --auth-google gmail --revoke      # gmail-read only
```

Each invocation calls Google's revoke endpoint for the specified scope
and deletes the matching local token file. Your OAuth client in Cloud
Console stays in place; only the granted consent is removed. To
re-authorize, run `claudette --auth-google <scope>` again.

You can also revoke from Google's side at
<https://myaccount.google.com/permissions>.

## Troubleshooting

**`invalid_scope` during consent.** Your OAuth consent screen is missing the
scope Claudette is requesting, OR the relevant API isn't enabled on the
project. Re-check step 2 (Calendar API enabled) and step 3 (OAuth consent
screen in Testing mode with your account as a test user).

**`redirect_uri_mismatch`.** You picked "Web application" instead of
"Desktop app" when creating the OAuth client. Desktop apps accept any
`http://127.0.0.1:<port>/...` redirect; web apps don't. Delete the client
and recreate it as Desktop.

**`response missing refresh_token`.** Happens when you re-authorize with an
already-granted account — Google only returns a refresh token on the very
first consent. Visit <https://myaccount.google.com/permissions>, revoke
Claudette, then rerun `--auth-google`.

**Callback page never loads.** Loopback may be blocked by a firewall or VPN.
Try disabling the VPN briefly for the first-time authorization; the refresh
token obtained will then work indefinitely without further loopback
connections.
