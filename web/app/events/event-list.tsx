"use client";

import Link from "next/link";
import type { User } from "oidc-client-ts";
import { useEffect, useState, useSyncExternalStore } from "react";

import { apiError, apiFetch } from "@/lib/api";
import { getUser, joinPath } from "@/lib/auth";
import {
  CALENDAR_PATH,
  type SiteEvent,
  eventAnchor,
  eventStructuredData,
  formatDate,
  formatTime,
  isOpen,
  loadEvents,
} from "@/lib/events";

function noSubscription(): () => void {
  return () => {};
}

type Load = { kind: "loading" } | { kind: "error"; message: string } | { kind: "ready" };

async function loadMyRsvps(): Promise<Set<string>> {
  const response = await apiFetch("/api/account/rsvps");
  if (!response.ok) {
    throw new Error(await apiError(response, "Couldn't load your RSVPs."));
  }
  const body = (await response.json()) as { events: string[] };
  return new Set(body.events);
}

function Rsvp({
  event,
  user,
  going,
  onChange,
}: {
  event: SiteEvent;
  user: User | null | undefined;
  going: boolean;
  onChange: (going: boolean, rsvps: number) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  if (!isOpen(event)) {
    return null;
  }
  if (!user) {
    return (
      <div className="actions">
        <Link className="button" href={joinPath(`/events/#${eventAnchor(event)}`)}>
          RSVP
        </Link>
      </div>
    );
  }

  async function change(next: boolean) {
    setBusy(true);
    setError(undefined);
    try {
      const response = await apiFetch(`/api/events/${event.id}/rsvp`, {
        method: next ? "PUT" : "DELETE",
      });
      if (!response.ok) {
        setError(await apiError(response, "Couldn't update your RSVP. Try again."));
        return;
      }
      const body = (await response.json()) as { going: boolean; rsvps: number };
      onChange(body.going, body.rsvps);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="actions">
        {going ? (
          <>
            <span className="going">You&apos;re going</span>
            <button
              type="button"
              className="link-button"
              disabled={busy}
              onClick={() => void change(false)}
            >
              {busy ? "Updating…" : "Can't make it"}
            </button>
          </>
        ) : (
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => void change(true)}
          >
            {busy ? "Saving…" : "I'm going"}
          </button>
        )}
      </div>
      {error && (
        <p className="account-note" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

function EventCard({
  event,
  user,
  going,
  onRsvp,
}: {
  event: SiteEvent;
  user: User | null | undefined;
  going: boolean;
  onRsvp: (going: boolean, rsvps: number) => void;
}) {
  const canceled = event.status === "canceled";
  return (
    <li id={eventAnchor(event)} className={canceled ? "event event-canceled" : "event"}>
      <p className="event-when">
        {formatDate(event.starts_at)} · {formatTime(event.starts_at)}
        {event.status === "active" && <span className="event-badge">Happening now</span>}
        {canceled && <span className="event-badge">Canceled</span>}
      </p>
      <h3>{event.name}</h3>
      {event.location && <p className="event-where">{event.location}</p>}
      {event.description && <p className="event-description">{event.description}</p>}
      {!canceled && (
        <p className="account-note">
          {event.rsvps === 1 ? "1 person going" : `${event.rsvps} people going`} ·{" "}
          <a href={event.url}>See it on Discord</a>
        </p>
      )}
      <Rsvp event={event} user={user} going={going} onChange={onRsvp} />
    </li>
  );
}

export function EventList() {
  const [load, setLoad] = useState<Load>({ kind: "loading" });
  const [events, setEvents] = useState<SiteEvent[]>([]);
  const [user, setUser] = useState<User | null>();
  const [mine, setMine] = useState<Set<string>>(new Set());
  // Calendar apps subscribe to webcal:// links; the prerendered page falls
  // back to the plain path until it knows its host.
  const calendarUrl = useSyncExternalStore(
    noSubscription,
    () => `webcal://${window.location.host}${CALENDAR_PATH}`,
    () => CALENDAR_PATH,
  );

  useEffect(() => {
    loadEvents().then(
      (loaded) => {
        setEvents(loaded);
        setLoad({ kind: "ready" });
      },
      (reason: unknown) => setLoad({ kind: "error", message: String(reason) }),
    );
    getUser()
      .catch(() => null)
      .then(async (signedIn) => {
        setUser(signedIn);
        if (signedIn) {
          setMine(await loadMyRsvps());
        }
      })
      .catch(() => setMine(new Set()));
  }, []);

  // Events render after loading, so the browser can't jump to a linked event
  // on its own.
  useEffect(() => {
    if (load.kind === "ready" && window.location.hash) {
      document.getElementById(window.location.hash.slice(1))?.scrollIntoView();
    }
  }, [load.kind]);

  function updateRsvp(event: SiteEvent, going: boolean, rsvps: number) {
    setEvents((current) => current.map((e) => (e.id === event.id ? { ...e, rsvps } : e)));
    setMine((current) => {
      const next = new Set(current);
      if (going) {
        next.add(event.id);
      } else {
        next.delete(event.id);
      }
      return next;
    });
  }

  const upcoming = events.filter((event) => event.upcoming);
  const past = events.filter((event) => !event.upcoming);

  return (
    <>
      <section className="event-section" aria-labelledby="upcoming-heading">
        <h2 id="upcoming-heading">Upcoming</h2>
        {load.kind === "loading" && <p className="account-note">Loading meetups…</p>}
        {load.kind === "error" && (
          <p className="account-note" role="alert">
            {load.message}
          </p>
        )}
        {load.kind === "ready" && upcoming.length === 0 && (
          <p className="member">
            Nothing on the calendar yet. Turn on meetup emails in your{" "}
            <Link href="/account/">account</Link> and we&apos;ll tell you when the next one is set.
          </p>
        )}
        {upcoming.length > 0 && (
          <>
            <script
              type="application/ld+json"
              // JSON.stringify of API data; "<" is escaped so it can't close the script tag.
              dangerouslySetInnerHTML={{
                __html: JSON.stringify(upcoming.map(eventStructuredData)).replace(/</g, "\\u003c"),
              }}
            />
            <ul className="event-list">
              {upcoming.map((event) => (
                <EventCard
                  key={event.id}
                  event={event}
                  user={user}
                  going={mine.has(event.id)}
                  onRsvp={(going, rsvps) => updateRsvp(event, going, rsvps)}
                />
              ))}
            </ul>
          </>
        )}
        <p className="account-note">
          <a href={calendarUrl}>Subscribe to the calendar</a> to see every meetup in Google, Apple,
          or Outlook Calendar, or <a href={CALENDAR_PATH}>download it</a>.
        </p>
      </section>

      {past.length > 0 && (
        <section className="event-section" aria-labelledby="past-heading">
          <h2 id="past-heading">Past meetups</h2>
          <ul className="event-list event-list-past">
            {past.map((event) => (
              <li key={event.id} id={eventAnchor(event)} className="event">
                <p className="event-when">{formatDate(event.starts_at)}</p>
                <h3>{event.name}</h3>
                {event.location && <p className="event-where">{event.location}</p>}
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}
