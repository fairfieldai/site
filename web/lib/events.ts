import { SITE_NAME, SITE_URL } from "./site";

/** A meetup from `GET /api/events`. Times are Unix seconds. */
export interface SiteEvent {
  id: string;
  name: string;
  description: string;
  starts_at: number;
  ends_at: number;
  location: string;
  url: string;
  status: "scheduled" | "active" | "ended" | "canceled";
  rsvps: number;
  /** Listed with upcoming events; canceled ones stay there until they'd have ended. */
  upcoming: boolean;
}

/** Path of the calendar feed. */
export const CALENDAR_PATH = "/api/events.ics";

export async function loadEvents(): Promise<SiteEvent[]> {
  const response = await fetch("/api/events", { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`Couldn't load events (HTTP ${response.status}).`);
  }
  const body = (await response.json()) as { events: SiteEvent[] };
  return body.events;
}

/** Whether members can RSVP. */
export function isOpen(event: SiteEvent): boolean {
  return event.upcoming && event.status !== "canceled";
}

/** The element ID of an event on the events page, linked from emails and the calendar feed. */
export function eventAnchor(event: SiteEvent): string {
  return `event-${event.id}`;
}

// Meetups are in Fairfield, so times are shown in its time zone.
const dateFormat = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/New_York",
  weekday: "long",
  month: "long",
  day: "numeric",
});
const timeFormat = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/New_York",
  hour: "numeric",
  minute: "2-digit",
  timeZoneName: "short",
});

export function formatDate(seconds: number): string {
  return dateFormat.format(new Date(seconds * 1000));
}

export function formatTime(seconds: number): string {
  return timeFormat.format(new Date(seconds * 1000));
}

/** schema.org description of an event, for search engines. */
export function eventStructuredData(event: SiteEvent) {
  const online = event.location.startsWith("Online");
  return {
    "@context": "https://schema.org",
    "@type": "Event",
    name: event.name,
    description: event.description || undefined,
    startDate: new Date(event.starts_at * 1000).toISOString(),
    endDate: new Date(event.ends_at * 1000).toISOString(),
    eventStatus:
      event.status === "canceled"
        ? "https://schema.org/EventCancelled"
        : "https://schema.org/EventScheduled",
    eventAttendanceMode: online
      ? "https://schema.org/OnlineEventAttendanceMode"
      : "https://schema.org/OfflineEventAttendanceMode",
    location: online
      ? { "@type": "VirtualLocation", url: event.url }
      : {
          "@type": "Place",
          name: event.location || "Fairfield, Connecticut",
          address: {
            "@type": "PostalAddress",
            addressLocality: "Fairfield",
            addressRegion: "CT",
            addressCountry: "US",
          },
        },
    isAccessibleForFree: true,
    organizer: { "@type": "Organization", name: SITE_NAME, url: `${SITE_URL}/` },
    url: `${SITE_URL}/events/#${eventAnchor(event)}`,
  };
}
