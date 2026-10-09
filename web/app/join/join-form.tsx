"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { type FormEvent, useEffect, useState } from "react";

import { getUser, safeReturnPath, signIn, storeTokens } from "@/lib/auth";
import { type JoinResult, JoinError, type PendingJoin, cognitoClient } from "@/lib/cognito";
import { loadConfig } from "@/lib/config";

type Step = { kind: "email" } | { kind: "code"; email: string; pending: PendingJoin };

async function client() {
  return cognitoClient(await loadConfig());
}

function message(reason: unknown): string {
  return reason instanceof JoinError ? reason.message : "Something went wrong. Try again.";
}

/** Where to go once signed in: the `next` query parameter, if it's a path on this site. */
function nextPath(): string {
  return safeReturnPath(new URLSearchParams(window.location.search).get("next"));
}

export function JoinForm() {
  const router = useRouter();
  const [step, setStep] = useState<Step>({ kind: "email" });
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [note, setNote] = useState<string>();

  useEffect(() => {
    getUser()
      .then((user) => {
        if (user) {
          router.replace(nextPath());
        }
      })
      .catch(() => {});
  }, [router]);

  async function handle(result: JoinResult, address: string, sent: string) {
    if (result.kind === "signed-in") {
      await storeTokens(result.tokens);
      router.replace(nextPath());
      return;
    }
    setStep({ kind: "code", email: address, pending: result.pending });
    setCode("");
    setNote(sent);
  }

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(undefined);
    try {
      await action();
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  }

  function sendCode(address: string) {
    return run(async () => {
      const result = await (await client()).startJoin(address);
      await handle(result, address, `We emailed a code to ${address}.`);
    });
  }

  function onSubmitEmail(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void sendCode(email.trim());
  }

  function onSubmitCode(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (step.kind !== "code") {
      return;
    }
    void run(async () => {
      const result = await (await client()).finishJoin(step.email, code, step.pending);
      await handle(
        result,
        step.email,
        `Your account is confirmed. We emailed one more code to ${step.email} to sign you in.`,
      );
    });
  }

  return (
    <div className="account">
      {step.kind === "email" ? (
        <form className="join-form" onSubmit={onSubmitEmail}>
          <label htmlFor="join-email">Email address</label>
          <input
            id="join-email"
            type="email"
            autoComplete="email"
            required
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
          <div className="actions">
            <button type="submit" className="button" disabled={busy}>
              {busy ? "Sending…" : "Email me a code"}
            </button>
          </div>
          <p className="account-note">
            By joining, you agree to our <Link href="/terms/">Terms</Link> and{" "}
            <Link href="/privacy/">Privacy Policy</Link>.
          </p>
          <p className="account-note">
            Already have a password or passkey?{" "}
            <button type="button" className="link-button" onClick={() => void signIn(nextPath())}>
              Sign in
            </button>
          </p>
        </form>
      ) : (
        <form className="join-form" onSubmit={onSubmitCode}>
          {note && <p className="member">{note}</p>}
          <label htmlFor="join-code">Code from the email</label>
          <input
            id="join-code"
            type="text"
            inputMode="numeric"
            autoComplete="one-time-code"
            required
            value={code}
            onChange={(event) => setCode(event.target.value)}
          />
          <div className="actions">
            <button type="submit" className="button" disabled={busy}>
              {busy ? "Checking…" : "Continue"}
            </button>
          </div>
          <p className="account-note">
            Didn&apos;t get it? Check your spam folder, or{" "}
            <button
              type="button"
              className="link-button"
              disabled={busy}
              onClick={() => void sendCode(step.email)}
            >
              send a new code
            </button>{" "}
            ·{" "}
            <button
              type="button"
              className="link-button"
              disabled={busy}
              onClick={() => {
                setStep({ kind: "email" });
                setError(undefined);
                setNote(undefined);
              }}
            >
              use a different email
            </button>
          </p>
        </form>
      )}
      {error && (
        <p className="account-note" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
