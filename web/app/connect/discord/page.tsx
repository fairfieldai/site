"use client";

import type { User } from "oidc-client-ts";
import Link from "next/link";
import { useEffect, useState } from "react";

import { apiError, apiFetch } from "@/lib/api";
import { getUser, signIn } from "@/lib/auth";
import { startDiscordConnect } from "@/lib/discord";
import { DISCORD_INVITE_URL } from "@/lib/links";

interface Status {
  linked: boolean;
  username?: string;
  scopes: string;
}

type View =
  | { kind: "loading" }
  | { kind: "signed-out" }
  | { kind: "unavailable" }
  | { kind: "ready"; user: User; status: Status }
  | { kind: "error"; message: string };

/** Works out what to show: sign-in, unavailable, or the member's link status. */
async function loadView(): Promise<View> {
  const user = await getUser().catch(() => null);
  if (!user) {
    return { kind: "signed-out" };
  }
  const response = await apiFetch("/api/account/discord");
  if (response.status === 404) {
    return { kind: "unavailable" };
  }
  if (!response.ok) {
    return {
      kind: "error",
      message: await apiError(response, "Couldn't load your Discord connection."),
    };
  }
  return { kind: "ready", user, status: (await response.json()) as Status };
}

function failure(reason: unknown): View {
  return { kind: "error", message: String(reason) };
}

export default function ConnectDiscord() {
  const [view, setView] = useState<View>({ kind: "loading" });
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    loadView().then(setView, (reason: unknown) => setView(failure(reason)));
  }, []);

  async function disconnect() {
    setBusy(true);
    const response = await apiFetch("/api/account/discord", { method: "DELETE" });
    const next = response.ok
      ? await loadView()
      : failure(await apiError(response, "Couldn't disconnect Discord."));
    setBusy(false);
    setView(next);
  }

  return (
    <main className="legal">
      <p className="eyebrow">Discord</p>
      <h1>Connect your Discord account</h1>
      <p className="lede">
        Link your Discord account to your fairfieldct.ai account to get the Member role in our{" "}
        <a href={DISCORD_INVITE_URL}>Discord server</a>.
      </p>
      <div className="account">
        {view.kind === "loading" && <p>Loading…</p>}
        {view.kind === "signed-out" && (
          <>
            <p>Sign in or create your fairfieldct.ai account first.</p>
            <div className="actions">
              <button
                type="button"
                className="button"
                onClick={() => void signIn("/connect/discord/")}
              >
                Sign in to connect
              </button>
            </div>
          </>
        )}
        {view.kind === "unavailable" && <p>Discord linking isn&apos;t available on this site.</p>}
        {view.kind === "error" && <p role="alert">{view.message}</p>}
        {view.kind === "ready" && view.status.linked && (
          <>
            <p className="member">
              Connected to Discord as <strong>@{view.status.username}</strong>.
            </p>
            <div className="actions">
              <button
                type="button"
                className="button button-secondary"
                disabled={busy}
                onClick={() => void startDiscordConnect(view.status.scopes)}
              >
                Switch Discord account
              </button>
              <button
                type="button"
                className="link-button"
                disabled={busy}
                onClick={() => void disconnect()}
              >
                {busy ? "Disconnecting…" : "Disconnect"}
              </button>
            </div>
          </>
        )}
        {view.kind === "ready" && !view.status.linked && (
          <>
            <p>Signed in as {view.user.profile.email}.</p>
            <div className="actions">
              <button
                type="button"
                className="button"
                onClick={() => void startDiscordConnect(view.status.scopes)}
              >
                Connect Discord
              </button>
            </div>
          </>
        )}
      </div>
      <p className="account-note">
        You can also disconnect any time in Discord under Settings → Authorized Apps. See our{" "}
        <Link href="/privacy/">Privacy Policy</Link>.
      </p>
    </main>
  );
}
