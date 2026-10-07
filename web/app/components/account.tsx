"use client";

import type { User } from "oidc-client-ts";
import { useEffect, useState } from "react";

import { getUser, signIn, signOut } from "@/lib/auth";

export function Account() {
  const [user, setUser] = useState<User | null>();

  useEffect(() => {
    getUser()
      .then(setUser)
      .catch(() => setUser(null));
  }, []);

  if (user === undefined) {
    // Holds the space so the page doesn't shift once the session loads.
    return <div className="account" aria-hidden="true" />;
  }

  if (!user) {
    return (
      <div className="account">
        <button type="button" className="button" onClick={() => void signIn()}>
          Join the community
        </button>
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
      <p className="account-note">
        <button type="button" className="link-button" onClick={() => void signOut()}>
          Sign out
        </button>
      </p>
    </div>
  );
}
