import type { Metadata } from "next";

import { pageMetadata } from "@/lib/site";

import { EventList } from "./event-list";

export const metadata: Metadata = pageMetadata({
  path: "/events/",
  title: "Meetups",
  description:
    "Upcoming and past fairfieldct.ai meetups in Fairfield, Connecticut. RSVP with your free account, get email reminders, or add the calendar to your phone.",
});

export default function Events() {
  return (
    <main className="events">
      <p className="eyebrow">Meetups</p>
      <h1>Come say hello.</h1>
      <p className="lede">
        Everyone&apos;s welcome, whatever you know about AI. RSVP so we know how many chairs to put
        out, and we&apos;ll email you a reminder the day before.
      </p>
      <EventList />
    </main>
  );
}
