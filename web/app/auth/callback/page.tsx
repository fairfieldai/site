"use client";

import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

import { completeSignIn } from "@/lib/auth";

export default function AuthCallback() {
  const router = useRouter();
  const [error, setError] = useState<string>();

  useEffect(() => {
    completeSignIn()
      .then((returnTo) => router.replace(returnTo))
      .catch((reason: unknown) => setError(String(reason)));
  }, [router]);

  return (
    <main className="hero">
      <h1>{error ? "Sign-in failed" : "Signing you in…"}</h1>
      {error && <p className="lede">{error}</p>}
    </main>
  );
}
