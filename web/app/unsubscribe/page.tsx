"use client";

import Link from "next/link";
import { useState, useSyncExternalStore } from "react";

type View =
  | { kind: "confirm" }
  | { kind: "working" }
  | { kind: "done"; unsubscribed: boolean }
  | { kind: "error"; message: string };

/**
 * The unsubscribe link in meetup emails. The token is in the fragment, which
 * browsers don't send to the server, and nothing changes until the member
 * confirms, so link scanners that open emailed URLs can't unsubscribe anyone.
 */
function subscribeToHash(onChange: () => void): () => void {
  window.addEventListener("hashchange", onChange);
  return () => window.removeEventListener("hashchange", onChange);
}

export default function Unsubscribe() {
  // null while prerendering, before the fragment can be read.
  const hash = useSyncExternalStore(
    subscribeToHash,
    () => window.location.hash.slice(1),
    () => null,
  );
  const token = hash !== null && /^[0-9a-f]{64}$/.test(hash) ? hash : undefined;
  const [view, setView] = useState<View>({ kind: "confirm" });

  async function unsubscribe(confirmed: string) {
    setView({ kind: "working" });
    try {
      const response = await fetch(`/api/email/unsubscribe/${confirmed}`, { method: "POST" });
      if (!response.ok) {
        setView({ kind: "error", message: "Something went wrong. Please try again." });
        return;
      }
      const body = (await response.json()) as { unsubscribed: boolean };
      setView({ kind: "done", unsubscribed: body.unsubscribed });
    } catch (reason) {
      setView({ kind: "error", message: String(reason) });
    }
  }

  return (
    <main className="legal">
      <p className="eyebrow">Email</p>
      <h1>Meetup emails</h1>
      <div className="account">
        {hash === null && <p>Loading…</p>}
        {hash !== null && !token && (
          <p>
            This unsubscribe link is incomplete. You can turn off meetup emails in your{" "}
            <Link href="/account/">account</Link>.
          </p>
        )}
        {token && view.kind === "confirm" && (
          <>
            <p>Stop getting emails about new meetups and day-before reminders?</p>
            <div className="actions">
              <button type="button" className="button" onClick={() => void unsubscribe(token)}>
                Unsubscribe
              </button>
            </div>
          </>
        )}
        {view.kind === "working" && <p>Unsubscribing…</p>}
        {view.kind === "done" && (
          <p className="member">
            {view.unsubscribed
              ? "You're unsubscribed. You'll still get reminders for meetups you RSVP to."
              : "You're already unsubscribed."}
          </p>
        )}
        {view.kind === "error" && <p role="alert">{view.message}</p>}
      </div>
      <p className="account-note">
        Changed your mind? Turn meetup emails back on in your <Link href="/account/">account</Link>.
      </p>
    </main>
  );
}
