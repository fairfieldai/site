import { User, UserManager } from "oidc-client-ts";

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

export async function signIn(): Promise<void> {
  await (await userManager()).signinRedirect();
}

export async function completeSignIn(): Promise<void> {
  await (await userManager()).signinRedirectCallback();
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
