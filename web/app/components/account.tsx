"use client";

import Link from "next/link";
import type { User } from "oidc-client-ts";
import { useEffect, useState } from "react";

import { apiFetch } from "@/lib/api";
import { getUser, joinPath, signIn, signOut } from "@/lib/auth";

/** The linked Discord username, or null when not linked or the status can't be loaded. */
async function linkedDiscordUsername(): Promise<string | null> {
  const response = await apiFetch("/api/account/discord");
  if (!response.ok) {
    return null;
  }
  const status = (await response.json()) as { linked: boolean; username?: string };
  return status.linked ? (status.username ?? null) : null;
}

export function Account() {
  const [user, setUser] = useState<User | null>();
  const [discordUsername, setDiscordUsername] = useState<string | null>(null);

  useEffect(() => {
    getUser()
      .then(setUser)
      .catch(() => setUser(null));
  }, []);

  useEffect(() => {
    if (!user) {
      return;
    }
    linkedDiscordUsername()
      .then(setDiscordUsername)
      .catch(() => setDiscordUsername(null));
  }, [user]);

  // Signed-out actions also render while the session loads, so they're in the
  // static HTML and visitors never see an empty space.
  if (!user) {
    return (
      <div className="account">
        <div className="actions">
          <Link className="button" href={joinPath()}>
            Join the community
          </Link>
        </div>
        <p className="account-note">
          Already joined?{" "}
          <button type="button" className="link-button" onClick={() => void signIn()}>
            Sign in
          </button>
        </p>
      </div>
    );
  }

  return (
    <div className="account">
      <p className="member">
        You&apos;re in, <strong>{user.profile.email}</strong>. See what&apos;s coming up on our{" "}
        <Link href="/events/">meetups page</Link>.
      </p>
      <p className="account-note">
        <Link href="/account/">Account and email settings</Link> ·{" "}
        <Link href="/connect/discord/">
          {discordUsername ? `Discord: @${discordUsername}` : "Connect your Discord account"}
        </Link>{" "}
        ·{" "}
        <button type="button" className="link-button" onClick={() => void signOut()}>
          Sign out
        </button>
      </p>
    </div>
  );
}
