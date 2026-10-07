"use client";

import Link from "next/link";
import type { User } from "oidc-client-ts";
import { useEffect, useState } from "react";

import { getUser, signIn, signOut } from "@/lib/auth";
import { DISCORD_INVITE_URL } from "@/lib/links";

function DiscordLink() {
  return (
    <a className="button button-secondary" href={DISCORD_INVITE_URL}>
      Join us on Discord
    </a>
  );
}

export function Account() {
  const [user, setUser] = useState<User | null>();

  useEffect(() => {
    getUser()
      .then(setUser)
      .catch(() => setUser(null));
  }, []);

  // Signed-out actions also render while the session loads, so they're in the
  // static HTML and visitors never see an empty space.
  if (!user) {
    return (
      <div className="account">
        <div className="actions">
          <button type="button" className="button" onClick={() => void signIn()}>
            Join the community
          </button>
          <DiscordLink />
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
        You&apos;re in, <strong>{user.profile.email}</strong>. Watch this space for the first
        events.
      </p>
      <div className="actions">
        <DiscordLink />
      </div>
      <p className="account-note">
        <Link href="/connect/discord/">Connect your Discord account</Link> ·{" "}
        <button type="button" className="link-button" onClick={() => void signOut()}>
          Sign out
        </button>
      </p>
    </div>
  );
}
