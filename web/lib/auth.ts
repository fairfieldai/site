import { type IdTokenClaims, User, UserManager } from "oidc-client-ts";

import { jwtClaims, type Tokens } from "./cognito";
import { loadConfig, type SiteConfig } from "./config";

let manager: Promise<UserManager> | undefined;

function createManager(config: SiteConfig): UserManager {
  const origin = window.location.origin;
  const domain = `https://${config.cognitoDomain}`;
  return new UserManager({
    authority: config.cognitoIssuer,
    // Cognito serves the OAuth endpoints from the managed login domain, not
    // the issuer, so list them instead of using discovery.
    metadata: {
      issuer: config.cognitoIssuer,
      authorization_endpoint: `${domain}/oauth2/authorize`,
      token_endpoint: `${domain}/oauth2/token`,
      userinfo_endpoint: `${domain}/oauth2/userInfo`,
      revocation_endpoint: `${domain}/oauth2/revoke`,
    },
    client_id: config.cognitoClientId,
    redirect_uri: `${origin}/auth/callback/`,
    post_logout_redirect_uri: `${origin}/`,
    response_type: "code",
    scope: "openid email profile",
    automaticSilentRenew: true,
  });
}

function userManager(): Promise<UserManager> {
  manager ??= loadConfig().then(createManager);
  return manager;
}

export async function getUser(): Promise<User | null> {
  const user = await (await userManager()).getUser();
  return user && !user.expired ? user : null;
}

/** A same-site path to return to after sign-in, never another origin. */
export function safeReturnPath(path: unknown): string {
  return typeof path === "string" && path.startsWith("/") && !path.startsWith("//") ? path : "/";
}

/** Signs in through Cognito, then returns to `returnTo` (a path on this site). */
export async function signIn(returnTo = "/"): Promise<void> {
  await (await userManager()).signinRedirect({ state: safeReturnPath(returnTo) });
}

/** The /join/ page, where visitors sign up or in with an emailed code, returning to `returnTo`. */
export function joinPath(returnTo = "/"): string {
  const path = safeReturnPath(returnTo);
  return path === "/" ? "/join/" : `/join/?next=${encodeURIComponent(path)}`;
}

/** Keeps the tokens from a /join/ sign-in, so the session works like one from managed login. */
export async function storeTokens(tokens: Tokens): Promise<void> {
  await (
    await userManager()
  ).storeUser(
    new User({
      id_token: tokens.IdToken,
      access_token: tokens.AccessToken,
      refresh_token: tokens.RefreshToken,
      token_type: tokens.TokenType,
      profile: jwtClaims(tokens.IdToken) as IdTokenClaims,
      expires_at: Math.floor(Date.now() / 1000) + tokens.ExpiresIn,
    }),
  );
}

/** Finishes sign-in and returns the path to go back to. */
export async function completeSignIn(): Promise<string> {
  const user = await (await userManager()).signinRedirectCallback();
  return safeReturnPath(user.state);
}

export async function signOut(): Promise<void> {
  const config = await loadConfig();
  await (await userManager()).removeUser();
  // Cognito's logout endpoint takes client_id and logout_uri rather than the
  // standard OIDC end-session parameters.
  const url = new URL(`https://${config.cognitoDomain}/logout`);
  url.searchParams.set("client_id", config.cognitoClientId);
  url.searchParams.set("logout_uri", `${window.location.origin}/`);
  window.location.assign(url);
}
