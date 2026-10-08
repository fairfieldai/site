"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

import {
  type SiteEvent,
  eventAnchor,
  formatDate,
  formatTime,
  isOpen,
  loadEvents,
} from "@/lib/events";

/** The next meetup, once one is scheduled. Shows nothing until then or if events can't load. */
export function NextMeetup() {
  const [next, setNext] = useState<SiteEvent>();

  useEffect(() => {
    loadEvents()
      .then((events) => setNext(events.find(isOpen)))
      .catch(() => setNext(undefined));
  }, []);

  if (!next) {
    return null;
  }
  return (
    <section className="next-meetup" aria-labelledby="next-meetup-heading">
      <p className="event-when">
        {next.status === "active" ? "Happening now" : "Next meetup"} · {formatDate(next.starts_at)}{" "}
        · {formatTime(next.starts_at)}
      </p>
      <h2 id="next-meetup-heading">{next.name}</h2>
      {next.location && <p className="event-where">{next.location}</p>}
      <Link className="button" href={`/events/#${eventAnchor(next)}`}>
        Details and RSVP
      </Link>
    </section>
  );
}
