# CLAUDE.md

Guidance for Claude Code in this repository.

## Layout

- `web/`: Next.js static export (`output: 'export'`, `trailingSlash: true`), pnpm, TypeScript, oxlint, oxfmt.
- `api/`: Cargo workspace of Rust Lambdas built for arm64 with cargo-lambda, which packages each binary as `target/lambda/<name>/bootstrap.zip`.
  - `crates/api`: the site API, an Axum app on `lambda_http` served under `/api`.
  - `crates/discord`: the Discord bot's interactions (`POST /api/discord/interactions`) and webhook events (`POST /api/discord/events`) endpoints. Every request's Ed25519 signature is checked against `DISCORD_PUBLIC_KEY` before parsing. Deployed only where the GitHub Environment sets `DISCORD_FUNCTION_NAME` (prod).
- `scripts/prune-static.sh`: removes stale `_next/static` objects after a deploy.
- Infrastructure (S3, CloudFront, API Gateway, Lambda, deploy roles, GitHub Environments) is Terraform in `fairfieldai/infra`, not here.

## Commands

```bash
cd web && pnpm format:check && pnpm lint && pnpm typecheck && pnpm build
cd api && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo deny check
prek run --all-files
```

## Constraints

- The site is static. No Route Handlers, Server Actions, middleware, ISR, or `next/image` optimization. Dynamic behavior goes in the API.
- API routes must be mounted under `/api`; CloudFront forwards the full path. Unknown routes return a JSON 404, but CloudFront replaces 404 bodies with the site's `404.html`, so clients should rely on the status code.
- The Lambda reads configuration from environment variables set by Terraform: `TABLE_NAME`, `SSM_PARAMETER_PATH`, `MAIL_API_BASE_URL`, `MAIL_API_KEY_PARAMETER`, `MAIL_FROM`, `COGNITO_ISSUER`, `COGNITO_CLIENT_ID`.
- Auth: Cognito managed login with a public app client (code flow + PKCE) through `oidc-client-ts` in `web/lib/auth.ts`. The site loads `/config.json` at runtime (written per environment by the deploy, or `web/public/config.json` locally). API handlers that take an `api::auth::User` argument require a valid Cognito access token (`Authorization: Bearer`); `api/crates/api/src/auth.rs` verifies it against the user pool JWKS, and the tests sign tokens with the test-only keys in `api/crates/api/testdata/`. Secrets are SecureString SSM parameters under `SSM_PARAMETER_PATH`.
- Email is sent through the AgentMail-compatible mailbox API at `MAIL_API_BASE_URL` as `hello@inbox.fairfieldct.ai`, authenticated with the bearer key in `MAIL_API_KEY_PARAMETER`.
- Rust dependencies: `api/Cargo.toml` declares each one once under `[workspace.dependencies]`, in alphabetical order, with an exact version (`=x.y.z`), `default-features = false`, and no features. Each crate's `Cargo.toml` uses `workspace = true` and enables only the features it needs.
- GitHub Actions are pinned to commit SHAs with a version comment.
