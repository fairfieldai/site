"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

import { apiError, apiFetch } from "@/lib/api";
import { consumeDiscordState } from "@/lib/discord";
import { DISCORD_INVITE_URL } from "@/lib/links";

type Result =
  | { kind: "working" }
  | { kind: "connected"; username: string }
  | { kind: "failed"; message: string };

async function finish(): Promise<Result> {
  const params = new URLSearchParams(window.location.search);
  if (!consumeDiscordState(params.get("state"))) {
    return {
      kind: "failed",
      message: "This link expired or didn't start here. Please try connecting again.",
    };
  }
  if (params.get("error")) {
    return { kind: "failed", message: "Discord didn't connect your account." };
  }
  const code = params.get("code");
  if (!code) {
    return { kind: "failed", message: "Discord didn't send an authorization code." };
  }
  const response = await apiFetch("/api/account/discord", {
    method: "POST",
    body: JSON.stringify({ code }),
  });
  if (!response.ok) {
    return {
      kind: "failed",
      message: await apiError(response, "Couldn't connect your Discord account."),
    };
  }
  const { username } = (await response.json()) as { username: string };
  return { kind: "connected", username };
}

export default function DiscordCallback() {
  const [result, setResult] = useState<Result>({ kind: "working" });

  useEffect(() => {
    // Drop the code from the address bar so a reload can't replay it.
    const done = finish();
    window.history.replaceState(null, "", window.location.pathname);
    done
      .then(setResult)
      .catch((reason: unknown) => setResult({ kind: "failed", message: String(reason) }));
  }, []);

  return (
    <main className="hero">
      {result.kind === "working" && <h1>Connecting Discord…</h1>}
      {result.kind === "connected" && (
        <>
          <h1>You&apos;re connected.</h1>
          <p className="lede">
            Your Discord account <strong>@{result.username}</strong> is linked. Head back to{" "}
            <a href={DISCORD_INVITE_URL}>our Discord server</a> to see your Member role.
          </p>
        </>
      )}
      {result.kind === "failed" && (
        <>
          <h1>Couldn&apos;t connect Discord</h1>
          <p className="lede" role="alert">
            {result.message} <Link href="/connect/discord/">Try again</Link>
          </p>
        </>
      )}
    </main>
  );
}
