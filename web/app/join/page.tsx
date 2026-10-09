import type { Metadata } from "next";

import { pageMetadata } from "@/lib/site";

import { JoinForm } from "./join-form";

export const metadata: Metadata = pageMetadata({
  path: "/join/",
  title: "Join",
  description:
    "Join fairfieldct.ai with just your email address. We'll email you a code, with no password to set.",
});

export default function Join() {
  return (
    <main className="legal">
      <p className="eyebrow">Join</p>
      <h1>Join the community</h1>
      <p className="lede">
        All we need is your email address. We&apos;ll send you a code to enter here, with no
        password to set. Once you&apos;re in, you can RSVP to meetups and choose which emails you
        get.
      </p>
      <JoinForm />
    </main>
  );
}
