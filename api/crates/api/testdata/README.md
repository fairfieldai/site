Test-only RSA keys for the token verification tests. They sign nothing outside
`cargo test` and are not secrets. `jwks.json` holds the public half of
`signing-key.pem` with key ID `test-key`; `other-key.pem` is not in it.
