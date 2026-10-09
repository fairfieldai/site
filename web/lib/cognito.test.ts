import { afterEach, describe, expect, it, vi } from "vitest";

import { JoinError, type Tokens, cognitoClient, jwtClaims } from "./cognito";

const ISSUER = "https://cognito-idp.us-east-1.amazonaws.com/us-east-1_test";
const TOKENS: Tokens = {
  IdToken: "id",
  AccessToken: "access",
  RefreshToken: "refresh",
  ExpiresIn: 3600,
  TokenType: "Bearer",
};

type Reply = { status?: number; body: unknown } | Error;

/** Stubs fetch with one reply per Cognito operation, in call order, and records the calls. */
function cognito(replies: Record<string, Reply[]>) {
  const calls: { operation: string; body: Record<string, unknown> }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init: RequestInit) => {
      expect(url).toBe("https://cognito-idp.us-east-1.amazonaws.com/");
      const headers = new Headers(init.headers);
      expect(headers.get("Content-Type")).toBe("application/x-amz-json-1.1");
      const operation =
        headers.get("X-Amz-Target")?.replace("AWSCognitoIdentityProviderService.", "") ?? "";
      calls.push({ operation, body: JSON.parse(String(init.body)) as Record<string, unknown> });
      const reply = replies[operation]?.shift();
      if (!reply) {
        throw new Error(`unexpected ${operation}`);
      }
      if (reply instanceof Error) {
        return Promise.reject(reply);
      }
      return Promise.resolve(
        new Response(JSON.stringify(reply.body), { status: reply.status ?? 200 }),
      );
    }),
  );
  return {
    calls,
    client: cognitoClient({ cognitoIssuer: ISSUER, cognitoClientId: "client" }),
  };
}

function failure(type: string): Reply {
  return { status: 400, body: { __type: type, message: "nope" } };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("startJoin", () => {
  it("signs up a new member without a password", async () => {
    const { client, calls } = cognito({ SignUp: [{ body: { Session: "s1" } }] });
    await expect(client.startJoin("  new@example.com ")).resolves.toEqual({
      kind: "code-sent",
      pending: { kind: "new", session: "s1" },
    });
    expect(calls).toEqual([
      {
        operation: "SignUp",
        body: {
          ClientId: "client",
          Username: "new@example.com",
          UserAttributes: [{ Name: "email", Value: "new@example.com" }],
        },
      },
    ]);
  });

  it("emails a sign-in code to an existing member", async () => {
    const { client, calls } = cognito({
      SignUp: [failure("UsernameExistsException")],
      InitiateAuth: [{ body: { ChallengeName: "EMAIL_OTP", Session: "s2" } }],
    });
    await expect(client.startJoin("old@example.com")).resolves.toEqual({
      kind: "code-sent",
      pending: { kind: "existing", session: "s2" },
    });
    expect(calls[1]?.body).toEqual({
      ClientId: "client",
      AuthFlow: "USER_AUTH",
      AuthParameters: { USERNAME: "old@example.com", PREFERRED_CHALLENGE: "EMAIL_OTP" },
    });
  });

  it("resends the confirmation code to a member who never confirmed", async () => {
    const { client, calls } = cognito({
      SignUp: [failure("UsernameExistsException")],
      InitiateAuth: [failure("UserNotConfirmedException")],
      ResendConfirmationCode: [{ body: {} }],
    });
    await expect(client.startJoin("half@example.com")).resolves.toEqual({
      kind: "code-sent",
      pending: { kind: "new" },
    });
    expect(calls.map((call) => call.operation)).toEqual([
      "SignUp",
      "InitiateAuth",
      "ResendConfirmationCode",
    ]);
  });

  it("namespaced error types are recognized", async () => {
    const { client } = cognito({
      SignUp: [failure("com.amazonaws.cognito#UsernameExistsException")],
      InitiateAuth: [{ body: { ChallengeName: "EMAIL_OTP", Session: "s2" } }],
    });
    await expect(client.startJoin("old@example.com")).resolves.toMatchObject({
      pending: { kind: "existing" },
    });
  });

  it("fails when Cognito offers a challenge other than an emailed code", async () => {
    const { client } = cognito({
      SignUp: [failure("UsernameExistsException")],
      InitiateAuth: [{ body: { ChallengeName: "SELECT_CHALLENGE", Session: "s2" } }],
    });
    await expect(client.startJoin("old@example.com")).rejects.toThrow(
      new JoinError("Something went wrong. Try again."),
    );
  });

  it.each([
    ["InvalidParameterException", "Enter a valid email address."],
    ["LimitExceededException", "Too many attempts. Wait a few minutes and try again."],
    ["TooManyRequestsException", "Too many attempts. Wait a few minutes and try again."],
    ["CodeDeliveryFailureException", "We couldn't email that address. Check it and try again."],
    ["InternalErrorException", "Something went wrong. Try again."],
  ])("explains %s", async (type, message) => {
    const { client } = cognito({ SignUp: [failure(type)] });
    await expect(client.startJoin("x@example.com")).rejects.toThrow(new JoinError(message));
  });

  it("explains errors from later steps", async () => {
    const { client } = cognito({
      SignUp: [failure("UsernameExistsException")],
      InitiateAuth: [failure("TooManyRequestsException")],
    });
    await expect(client.startJoin("old@example.com")).rejects.toThrow(
      new JoinError("Too many attempts. Wait a few minutes and try again."),
    );
  });

  it("handles an error response without a JSON body", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response("<html>", { status: 502 }))),
    );
    const client = cognitoClient({ cognitoIssuer: ISSUER, cognitoClientId: "client" });
    await expect(client.startJoin("x@example.com")).rejects.toThrow(
      new JoinError("Something went wrong. Try again."),
    );
  });

  it("explains a network failure", async () => {
    const { client } = cognito({ SignUp: [new TypeError("Failed to fetch")] });
    await expect(client.startJoin("x@example.com")).rejects.toThrow(
      new JoinError("Couldn't reach the sign-in service. Check your connection and try again."),
    );
  });
});

describe("finishJoin", () => {
  it("confirms a new member and signs them in with the session", async () => {
    const { client, calls } = cognito({
      ConfirmSignUp: [{ body: { Session: "s3" } }],
      InitiateAuth: [{ body: { AuthenticationResult: TOKENS } }],
    });
    await expect(
      client.finishJoin("new@example.com", " 123456 ", { kind: "new", session: "s1" }),
    ).resolves.toEqual({ kind: "signed-in", tokens: TOKENS });
    expect(calls).toEqual([
      {
        operation: "ConfirmSignUp",
        body: {
          ClientId: "client",
          Username: "new@example.com",
          ConfirmationCode: "123456",
          Session: "s1",
        },
      },
      {
        operation: "InitiateAuth",
        body: {
          ClientId: "client",
          AuthFlow: "USER_AUTH",
          AuthParameters: { USERNAME: "new@example.com" },
          Session: "s3",
        },
      },
    ]);
  });

  it("emails a sign-in code when confirming returns no session", async () => {
    const { client } = cognito({
      ConfirmSignUp: [{ body: {} }],
      InitiateAuth: [{ body: { ChallengeName: "EMAIL_OTP", Session: "s4" } }],
    });
    await expect(client.finishJoin("half@example.com", "123456", { kind: "new" })).resolves.toEqual(
      { kind: "code-sent", pending: { kind: "existing", session: "s4" } },
    );
  });

  it("signs in an existing member with the emailed code", async () => {
    const { client, calls } = cognito({
      RespondToAuthChallenge: [{ body: { AuthenticationResult: TOKENS } }],
    });
    await expect(
      client.finishJoin("old@example.com", "654321", { kind: "existing", session: "s2" }),
    ).resolves.toEqual({ kind: "signed-in", tokens: TOKENS });
    expect(calls[0]?.body).toEqual({
      ClientId: "client",
      ChallengeName: "EMAIL_OTP",
      Session: "s2",
      ChallengeResponses: { USERNAME: "old@example.com", EMAIL_OTP_CODE: "654321" },
    });
  });

  it("fails when signing in returns no tokens", async () => {
    const { client } = cognito({
      RespondToAuthChallenge: [{ body: { ChallengeName: "EMAIL_OTP", Session: "s5" } }],
    });
    await expect(
      client.finishJoin("old@example.com", "654321", { kind: "existing", session: "s2" }),
    ).rejects.toThrow(new JoinError("Something went wrong. Try again."));
  });

  it.each([
    ["CodeMismatchException", "That code isn't right. Check the email and try again."],
    ["ExpiredCodeException", "That code has expired. Send a new one."],
    ["NotAuthorizedException", "That code has expired. Send a new one."],
    ["TooManyFailedAttemptsException", "Too many attempts. Wait a few minutes and try again."],
  ])("explains %s", async (type, message) => {
    const { client } = cognito({ RespondToAuthChallenge: [failure(type)] });
    await expect(
      client.finishJoin("old@example.com", "000000", { kind: "existing", session: "s2" }),
    ).rejects.toThrow(new JoinError(message));
  });
});

describe("jwtClaims", () => {
  function token(claims: unknown): string {
    const payload = Buffer.from(JSON.stringify(claims)).toString("base64url");
    return `header.${payload}.signature`;
  }

  it("decodes base64url payloads, including non-ASCII text", () => {
    const claims = { sub: "u1", email: "zoë@example.com", "?>": "~~~" };
    expect(jwtClaims(token(claims))).toEqual(claims);
  });

  it.each(["", "no-dots", "header."])("rejects the malformed token %j", (bad) => {
    expect(() => jwtClaims(bad)).toThrow("Malformed token");
  });
});
