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
    return null;
  }

  if (!user) {
    return (
      <p>
        <button type="button" onClick={() => void signIn()}>
          Sign in or create an account
        </button>
      </p>
    );
  }

  return (
    <p>
      Signed in as {user.profile.email}.{" "}
      <button type="button" onClick={() => void signOut()}>
        Sign out
      </button>
    </p>
  );
}
