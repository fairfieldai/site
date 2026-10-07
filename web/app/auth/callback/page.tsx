"use client";

import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

import { completeSignIn } from "@/lib/auth";

export default function AuthCallback() {
  const router = useRouter();
  const [error, setError] = useState<string>();

  useEffect(() => {
    completeSignIn()
      .then(() => router.replace("/"))
      .catch((reason: unknown) => setError(String(reason)));
  }, [router]);

  return (
    <main>
      <h1>{error ? "Sign-in failed" : "Signing in…"}</h1>
      {error && <p>{error}</p>}
    </main>
  );
}
