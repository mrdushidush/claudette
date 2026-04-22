# Security policy

Thank you for helping keep Claudette and its users safe.

## Supported versions

Only the latest released version on `main` is actively supported. We
do not backport fixes to prior minor versions.

| Version | Supported |
|---------|-----------|
| `v0.2.x` | ✅        |
| `< 0.2`  | ❌        |

## Reporting a vulnerability

**Please do not open a public GitHub issue for security reports.**
Open issues are visible to everyone and give attackers a head start.

Instead, file a private report through GitHub's security advisory
system:

1. Go to <https://github.com/mrdushidush/claudette/security/advisories/new>
2. Fill in the form — include a description, affected versions, a
   reproducer if you have one, and your estimate of impact.
3. Submit. Only the repo maintainers will see it.

If for any reason the security advisory flow is unavailable, email
the repo owner at the address listed on the GitHub profile page.

## What to expect

- **Acknowledgement:** within 7 days (often same-day — this is a
  solo-maintainer project, so responsiveness depends on other
  demands).
- **Triage:** confirmation that we can reproduce, plus initial
  severity assessment, within 14 days.
- **Fix:** timeline depends on severity. Critical issues are
  prioritised above all other work; medium issues land in the next
  scheduled release; low issues may be deferred and documented.
- **Disclosure:** coordinated. We'll work with you on a public
  disclosure date. Credit in the release notes if you'd like it.

## Scope

In scope:

- Vulnerabilities in Claudette's own code (anything under `src/`).
- Incorrect handling of secrets (tokens, OAuth credentials).
- Prompt-injection attacks that escape the `<email>` provenance
  boundary or otherwise cause Claudette to act against user intent
  via hostile tool input.
- Permission-tier bypasses (a `ReadOnly` tool performing writes, a
  `DangerFullAccess` tool executing without the confirmation prompt).
- Sandbox escapes from the `~/.claudette/files/` scratch directory.

Out of scope:

- Vulnerabilities in upstream dependencies. Report those to the
  upstream project; we'll bump the dep version once a fix is
  available.
- Vulnerabilities in Ollama, Whisper, edge-tts, the Telegram Bot API,
  or Google's APIs. Those are separate surfaces we consume.
- Denial-of-service attacks against the local process. Claudette is
  a single-user tool; if someone has the ability to spam your local
  Ollama, they already have code execution on your machine.
- Issues that require the attacker to already control the user's
  machine (e.g. tampering with `~/.claudette/secrets/` files).

## Threat model we target

Claudette is designed for a single-user, local-deployment threat
model:

1. The user trusts their own machine.
2. The user trusts themselves and any personas / memory files they
   load into Claudette.
3. External content (emails fetched via `gmail_read`, web pages
   fetched via `web_fetch`, RSVP data from Calendar) is untrusted
   and must be handled as data, not as instructions.

The `<email>` provenance wrapping in the Gmail tool group (see
`docs/sprint_life_agent.md` AD-6) is the primary defense against
prompt injection. If you can bypass it, that is a security-relevant
bug we want to hear about.

## Non-goals

- **Multi-tenant safety.** Claudette is not designed for shared
  hosting. Each user runs their own instance.
- **Adversarial model providers.** We trust Ollama and the model
  weights. If a malicious Ollama server returns crafted output,
  that's equivalent to trusting the brain — outside this project's
  scope.
- **Network interception.** Transport security is whatever
  `reqwest` / `rustls` give us. We don't pin certificates for the
  external APIs we call.

## History

This is the first version of the security policy; no reported
vulnerabilities yet. If that changes, resolved issues will be listed
here with CVE references if applicable.
