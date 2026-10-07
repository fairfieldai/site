# CLAUDE.md

Guidance for Claude Code in this repository.

## Layout

- `web/`: Next.js static export (`output: 'export'`, `trailingSlash: true`), pnpm, TypeScript, oxlint, oxfmt.
- `api/`: Rust Axum app on `lambda_http`, built for Lambda arm64 with cargo-lambda. The `bootstrap` binary wraps `api::router()` from `src/lib.rs`.
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
- The Lambda reads configuration from environment variables set by Terraform: `TABLE_NAME`, `SSM_PARAMETER_PATH`, `MAIL_API_BASE_URL`, `MAIL_API_KEY_PARAMETER`, `MAIL_FROM`. Secrets are SecureString SSM parameters under `SSM_PARAMETER_PATH`.
- Email is sent through the AgentMail-compatible mailbox API at `MAIL_API_BASE_URL` as `hello@inbox.fairfieldct.ai`, authenticated with the bearer key in `MAIL_API_KEY_PARAMETER`.
- GitHub Actions are pinned to commit SHAs with a version comment.
