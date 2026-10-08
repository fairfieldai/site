"use client";

import Link from "next/link";
import type { User } from "oidc-client-ts";
import { useEffect, useState } from "react";

import { apiError, apiFetch } from "@/lib/api";
import { getUser, signIn, signOut } from "@/lib/auth";

type View =
  | { kind: "loading" }
  | { kind: "signed-out" }
  | { kind: "ready"; user: User; announcements: boolean }
  | { kind: "error"; message: string };

async function loadView(): Promise<View> {
  const user = await getUser().catch(() => null);
  if (!user) {
    return { kind: "signed-out" };
  }
  const response = await apiFetch("/api/account/email");
  if (!response.ok) {
    return {
      kind: "error",
      message: await apiError(response, "Couldn't load your email settings."),
    };
  }
  const body = (await response.json()) as { announcements: boolean };
  return { kind: "ready", user, announcements: body.announcements };
}

export default function Account() {
  const [view, setView] = useState<View>({ kind: "loading" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    loadView().then(setView, (reason: unknown) =>
      setView({ kind: "error", message: String(reason) }),
    );
  }, []);

  async function setAnnouncements(user: User, announcements: boolean) {
    setBusy(true);
    setError(undefined);
    try {
      const response = await apiFetch("/api/account/email", {
        method: "PUT",
        body: JSON.stringify({ announcements }),
      });
      if (!response.ok) {
        setError(await apiError(response, "Couldn't save your email settings."));
        return;
      }
      const body = (await response.json()) as { announcements: boolean };
      setView({ kind: "ready", user, announcements: body.announcements });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="legal">
      <p className="eyebrow">Account</p>
      <h1>Your account</h1>
      <div className="account">
        {view.kind === "loading" && <p>Loading…</p>}
        {view.kind === "error" && <p role="alert">{view.message}</p>}
        {view.kind === "signed-out" && (
          <>
            <p>Sign in to manage your email and Discord settings.</p>
            <div className="actions">
              <button type="button" className="button" onClick={() => void signIn("/account/")}>
                Sign in
              </button>
            </div>
          </>
        )}
        {view.kind === "ready" && (
          <>
            <p className="member">
              Signed in as <strong>{view.user.profile.email}</strong>.
            </p>

            <h2>Email</h2>
            <label className="toggle">
              <input
                type="checkbox"
                checked={view.announcements}
                disabled={busy}
                onChange={(event) => void setAnnouncements(view.user, event.target.checked)}
              />
              <span>
                Email me when a new meetup is scheduled, and the day before each one.
                <span className="account-note">
                  Meetups you RSVP to always get a reminder the day before and a note if
                  they&apos;re canceled.
                </span>
              </span>
            </label>
            {error && <p role="alert">{error}</p>}

            <h2>Discord</h2>
            <p>
              Link your Discord account to get the Member role in our server.{" "}
              <Link href="/connect/discord/">Manage your Discord connection</Link>
            </p>

            <h2>Meetups</h2>
            <p>
              <Link href="/events/">See upcoming meetups</Link> and RSVP.
            </p>

            <div className="actions account-actions">
              <button
                type="button"
                className="button button-secondary"
                onClick={() => void signOut()}
              >
                Sign out
              </button>
            </div>
          </>
        )}
      </div>
    </main>
  );
}
