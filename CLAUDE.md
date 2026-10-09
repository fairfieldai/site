# CLAUDE.md

Guidance for Claude Code in this repository.

## Layout

- `web/`: Next.js static export (`output: 'export'`, `trailingSlash: true`), pnpm, TypeScript, oxlint, oxfmt.
- `api/`: Cargo workspace of Rust Lambdas built for arm64 with cargo-lambda, which packages each binary as `target/lambda/<name>/bootstrap.zip`.
  - `crates/api`: the site API, an Axum app on `lambda_http` served under `/api`.
  - `crates/shared`: SSM parameters, time helpers, and the DynamoDB stores used by both Lambdas: Discord account links, events and RSVPs (`shared::events`), and meetup email subscribers (`shared::subscribers`).
  - `crates/discord`: the Discord bot's interactions (`POST /api/discord/interactions`) and webhook events (`POST /api/discord/events`) endpoints. Every request's Ed25519 signature is checked against `DISCORD_PUBLIC_KEY` before parsing. `/meetup` replies with the next Discord scheduled event (the server's Events tab is the source of truth), reading it with the bot token from SSM, which is fetched on first use.
  - `crates/discord` also builds `discord-reminders`, run by EventBridge Scheduler every 15 minutes with `{"scheduled_time": …}`. It posts new-meetup announcements and one-week, one-day, and one-hour reminders to `#announcements` through a webhook (`DISCORD_ANNOUNCEMENTS_WEBHOOK_PARAMETER`). Each run covers the interval ending at its scheduled time (`reminders::due`), so every post happens exactly once with no stored state. All posts set `allowed_mentions: {parse: []}`.
  - The same job (`discord::site`) copies Discord's scheduled events into every table in `TABLE_NAMES`, because Discord drops events once they end; events missing from Discord are closed as canceled (not yet started) or ended. Where `EMAIL_TABLE_NAME` is set (prod), it emails that site's members through the mailbox API: new-meetup announcements and day-before reminders to subscribers, and day-before reminders and cancellations to members who RSVP'd. Addresses come from Cognito (`AdminGetUser`, verified addresses only) at send time. Each email has an `Idempotency-Key`, and sync or email failures are logged without failing the run, since a retried run would repeat its Discord posts. Cancellation emails go out before the event is closed, so a failed send is retried by the next run.
  - Both deploy only where the GitHub Environment sets `DISCORD_FUNCTION_NAME` / `DISCORD_REMINDERS_FUNCTION_NAME` (prod).
- `scripts/prune-static.sh`: removes stale `_next/static` objects after a deploy.
- Infrastructure (S3, CloudFront, API Gateway, Lambda, deploy roles, GitHub Environments) is Terraform in `fairfieldai/infra`, not here.

## Commands

```bash
cd web && pnpm format:check && pnpm lint && pnpm typecheck && pnpm test && pnpm build
cd api && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo deny check
prek run --all-files
```

## Constraints

- The site is static. No Route Handlers, Server Actions, middleware, ISR, or `next/image` optimization. Dynamic behavior goes in the API.
- API routes must be mounted under `/api`; CloudFront forwards the full path. Unknown routes return a JSON 404, but CloudFront replaces 404 bodies with the site's `404.html`, so clients should rely on the status code.
- The Lambda reads configuration from environment variables set by Terraform: `TABLE_NAME`, `SSM_PARAMETER_PATH`, `MAIL_API_BASE_URL`, `MAIL_API_KEY_PARAMETER`, `MAIL_FROM`, `COGNITO_ISSUER`, `COGNITO_CLIENT_ID`, `SITE_URL`.
- Meetups: Discord scheduled events are the source of truth; the site shows the copy the reminder job keeps in DynamoDB, so it lags Discord by up to 15 minutes. `GET /api/events` and `GET /api/events.ics` (the calendar feed) are public; `PUT`/`DELETE /api/events/{id}/rsvp` and `GET /api/account/rsvps` need sign-in. `/events/` lists them, with each event at `#event-<id>`.
- Meetup emails: members opt in at `/account/` (`GET`/`PUT /api/account/email`). Each subscriber gets a random unsubscribe token; emails link to `/unsubscribe/#<token>` (a fragment, so it stays out of server logs) and carry RFC 8058 one-click headers pointing at `POST /api/email/unsubscribe/{token}`.
- Auth: Cognito managed login with a public app client (code flow + PKCE) through `oidc-client-ts` in `web/lib/auth.ts`. Managed login always asks for a password at sign-up, so Join goes to `/join/` instead, which signs members up and in with an emailed code through Cognito's public API (`web/lib/cognito.ts`: `SignUp` with no password, `ConfirmSignUp`, `InitiateAuth` `USER_AUTH`) and stores the tokens with `storeTokens`. The site loads `/config.json` at runtime (written per environment by the deploy, or `web/public/config.json` locally). API handlers that take an `api::auth::User` argument require a valid Cognito access token (`Authorization: Bearer`); `api/crates/api/src/auth.rs` verifies it against the user pool JWKS, and the tests sign tokens with the test-only keys in `api/crates/api/testdata/`. Secrets are SecureString SSM parameters under `SSM_PARAMETER_PATH`.
- Email is sent through the AgentMail-compatible mailbox API at `MAIL_API_BASE_URL` as `hello@inbox.fairfieldct.ai`, authenticated with the bearer key in `MAIL_API_KEY_PARAMETER`.
- Discord account linking (Linked Roles): `/connect/discord/` sends the member through Discord's consent screen (scopes come from `GET /api/account/discord`; `state` lives in `sessionStorage`), and `/connect/discord/callback/` posts the code to `POST /api/account/discord`. `api/crates/api/src/discord_link.rs` exchanges it, stores the link (`shared::links`, two DynamoDB items so it can be found from either side, one Discord account per site account), and, where `DISCORD_MANAGE_ROLE_CONNECTION=true` (prod only, since prod and dev share one Discord application), sets the role connection. `DELETE` refreshes Discord's token (saving the rotated one first), clears the role connection, revokes, and unlinks. The Discord function removes links from every table in `TABLE_NAMES` on `APPLICATION_DEAUTHORIZED`.
- Rust dependencies: `api/Cargo.toml` declares each one once under `[workspace.dependencies]`, in alphabetical order, with an exact version (`=x.y.z`), `default-features = false`, and no features. Each crate's `Cargo.toml` uses `workspace = true` and enables only the features it needs.
- GitHub Actions are pinned to commit SHAs with a version comment.
