# fairfieldct.ai site

The fairfieldct.ai website: a statically exported Next.js site and a Rust API.
Infrastructure lives in [fairfieldai/infra](https://github.com/fairfieldai/infra).

| Path | Contents |
|------|----------|
| `web/` | Next.js app built with `output: 'export'`, served from S3 behind CloudFront |
| `api/` | Axum app running on Lambda (`provided.al2023`, arm64) behind API Gateway, served under `/api` |
| `scripts/` | Deploy helpers |
| `.github/workflows/` | CI and deploys |

## Development

Requires the latest Node, pnpm, and stable Rust.

```sh
cd web && pnpm install && pnpm dev    # http://localhost:3000
cd api && cargo test
```

Sign-in uses Cognito managed login. For `pnpm dev`, point the site at the dev
user pool by creating `web/public/config.json` (gitignored); the values are the
`dev` Terraform outputs in fairfieldai/infra:

```json
{
  "cognitoDomain": "auth.dev.fairfieldct.ai",
  "cognitoClientId": "<cognito_client_id>",
  "cognitoIssuer": "https://cognito-idp.us-east-1.amazonaws.com/<cognito_user_pool_id>"
}
```

The site is a static export: no Route Handlers, Server Actions, middleware, or
ISR. Put server logic in the API. Every API route lives under `/api`, because
CloudFront forwards the full path.

Install the git hooks with [prek](https://github.com/j178/prek):

```sh
prek install
```

## Deploys

A push to `main` that touches `web/`, `api/`, or `scripts/` builds the site
and the Lambda once, deploys both to dev, then waits for approval in the `prod`
GitHub Environment before deploying the same build to prod. A newer push
cancels builds still in progress, but never a deploy that has started. A deploy
only runs if its commit is still the tip of `main`, so approving an older run
can't overwrite a newer one; roll back by reverting on `main`. Each deploy:

1. Writes `config.json` with that environment's Cognito settings.
2. Updates the Lambda code.
3. Uploads `_next/static` with a one-year immutable cache, then everything else
   with `max-age=0, must-revalidate`, deleting removed pages.
4. Invalidates CloudFront.
5. Deletes `_next/static` files missing from the build and last uploaded more
   than 7 days ago (`scripts/prune-static.sh`).

The deploy role and the `AWS_ROLE_ARN`, `AWS_REGION`, `S3_BUCKET`,
`CLOUDFRONT_DISTRIBUTION_ID`, `LAMBDA_FUNCTION_NAME`, `COGNITO_DOMAIN`,
`COGNITO_CLIENT_ID`, and `COGNITO_ISSUER` Environment variables
come from Terraform in fairfieldai/infra.
