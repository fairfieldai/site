// Passwordless joining through Cognito's public user pool API. Managed login
// always asks for a password at sign-up, but the API doesn't when the pool
// allows emailed one-time codes, so /join/ signs members up and in with just
// their email address.

/** Tokens from a completed sign-in. */
export interface Tokens {
  IdToken: string;
  AccessToken: string;
  RefreshToken?: string;
  ExpiresIn: number;
  TokenType: string;
}

/** A join waiting for the code we emailed. */
export type PendingJoin =
  // A new member, or one who signed up but never confirmed: the code confirms the account.
  | { kind: "new"; session?: string }
  // A confirmed member: the code signs them in.
  | { kind: "existing"; session: string };

export type JoinResult =
  | { kind: "signed-in"; tokens: Tokens }
  | { kind: "code-sent"; pending: PendingJoin };

/** A failure to show the visitor as is. */
export class JoinError extends Error {}

const MESSAGES: Record<string, string> = {
  CodeMismatchException: "That code isn't right. Check the email and try again.",
  ExpiredCodeException: "That code has expired. Send a new one.",
  InvalidParameterException: "Enter a valid email address.",
  LimitExceededException: "Too many attempts. Wait a few minutes and try again.",
  TooManyRequestsException: "Too many attempts. Wait a few minutes and try again.",
  TooManyFailedAttemptsException: "Too many attempts. Wait a few minutes and try again.",
  NotAuthorizedException: "That code has expired. Send a new one.",
  CodeDeliveryFailureException: "We couldn't email that address. Check it and try again.",
};
const FALLBACK = "Something went wrong. Try again.";

class CognitoError extends Error {
  constructor(readonly type: string) {
    super(type);
  }
}

interface AuthResponse {
  AuthenticationResult?: Tokens;
  ChallengeName?: string;
  Session?: string;
}

export function cognitoClient({
  cognitoIssuer,
  cognitoClientId,
}: {
  cognitoIssuer: string;
  cognitoClientId: string;
}) {
  // The issuer is https://cognito-idp.<region>.amazonaws.com/<pool id>, and the
  // API is served from its origin.
  const endpoint = `${new URL(cognitoIssuer).origin}/`;

  async function call<T>(operation: string, body: Record<string, unknown>): Promise<T> {
    let response: Response;
    try {
      response = await fetch(endpoint, {
        method: "POST",
        headers: {
          "Content-Type": "application/x-amz-json-1.1",
          "X-Amz-Target": `AWSCognitoIdentityProviderService.${operation}`,
        },
        body: JSON.stringify({ ClientId: cognitoClientId, ...body }),
      });
    } catch {
      throw new JoinError(
        "Couldn't reach the sign-in service. Check your connection and try again.",
      );
    }
    const payload = (await response.json().catch(() => ({}))) as { __type?: string };
    if (!response.ok) {
      // Some responses prefix the type with a namespace ("…#UsernameExistsException").
      // oxlint-disable-next-line no-underscore-dangle -- Cognito names the error field `__type`.
      throw new CognitoError(payload.__type?.split("#").pop() ?? "UnknownError");
    }
    return payload as T;
  }

  function signedIn(response: AuthResponse): JoinResult {
    if (!response.AuthenticationResult) {
      throw new JoinError(FALLBACK);
    }
    return { kind: "signed-in", tokens: response.AuthenticationResult };
  }

  /** Emails a code that signs in an existing, confirmed member. */
  async function emailSignInCode(email: string): Promise<JoinResult> {
    const response = await call<AuthResponse>("InitiateAuth", {
      AuthFlow: "USER_AUTH",
      AuthParameters: { USERNAME: email, PREFERRED_CHALLENGE: "EMAIL_OTP" },
    });
    if (response.ChallengeName !== "EMAIL_OTP" || !response.Session) {
      throw new JoinError(FALLBACK);
    }
    return { kind: "code-sent", pending: { kind: "existing", session: response.Session } };
  }

  async function start(email: string): Promise<JoinResult> {
    try {
      const response = await call<{ Session?: string }>("SignUp", {
        Username: email,
        UserAttributes: [{ Name: "email", Value: email }],
      });
      return { kind: "code-sent", pending: { kind: "new", session: response.Session } };
    } catch (error) {
      if (!(error instanceof CognitoError) || error.type !== "UsernameExistsException") {
        throw error;
      }
    }
    try {
      return await emailSignInCode(email);
    } catch (error) {
      if (!(error instanceof CognitoError) || error.type !== "UserNotConfirmedException") {
        throw error;
      }
    }
    await call("ResendConfirmationCode", { Username: email });
    return { kind: "code-sent", pending: { kind: "new" } };
  }

  async function finish(email: string, code: string, pending: PendingJoin): Promise<JoinResult> {
    if (pending.kind === "existing") {
      return signedIn(
        await call<AuthResponse>("RespondToAuthChallenge", {
          ChallengeName: "EMAIL_OTP",
          Session: pending.session,
          ChallengeResponses: { USERNAME: email, EMAIL_OTP_CODE: code },
        }),
      );
    }
    const confirmed = await call<{ Session?: string }>("ConfirmSignUp", {
      Username: email,
      ConfirmationCode: code,
      Session: pending.session,
    });
    // Cognito returns a session to sign in with only when sign-up started
    // with one; otherwise the confirmed member needs a sign-in code.
    if (!confirmed.Session) {
      return emailSignInCode(email);
    }
    return signedIn(
      await call<AuthResponse>("InitiateAuth", {
        AuthFlow: "USER_AUTH",
        AuthParameters: { USERNAME: email },
        Session: confirmed.Session,
      }),
    );
  }

  /** Wraps Cognito error codes in messages for visitors. */
  function friendly<A extends unknown[]>(
    step: (...args: A) => Promise<JoinResult>,
  ): (...args: A) => Promise<JoinResult> {
    return async (...args) => {
      try {
        return await step(...args);
      } catch (error) {
        if (error instanceof CognitoError) {
          throw new JoinError(MESSAGES[error.type] ?? FALLBACK);
        }
        throw error;
      }
    };
  }

  return {
    /** Emails a code to a new or existing member. */
    startJoin: friendly((email: string) => start(email.trim())),
    /** Checks the emailed code; signs in, or emails one more code to finish signing in. */
    finishJoin: friendly((email: string, code: string, pending: PendingJoin) =>
      finish(email.trim(), code.trim(), pending),
    ),
  };
}

/** The claims in a JWT's payload. The token came straight from Cognito over TLS, so it isn't verified here. */
export function jwtClaims(token: string): Record<string, unknown> {
  const payload = token.split(".")[1];
  if (!payload) {
    throw new Error("Malformed token");
  }
  const base64 = payload.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64.padEnd(base64.length + ((4 - (base64.length % 4)) % 4), "="));
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
  return JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>;
}
