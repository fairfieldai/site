import type { Metadata } from "next";

import { DISCORD_INVITE_URL } from "./links";

// Canonical production origin, the same in every environment: dev serves this
// build too, and its pages name these URLs as canonical so search engines list
// the www pages.
export const SITE_URL = "https://www.fairfieldct.ai";
export const SITE_NAME = "fairfieldct.ai";
export const SITE_DESCRIPTION =
  "Neighbors in Fairfield, Southport, Westport, Easton, and Trumbull, Connecticut, figuring out AI together. Meetups, demo nights, and hands-on learning. No expertise required.";
export const CONTACT_EMAIL = "hello@inbox.fairfieldct.ai";
export const GITHUB_URL = "https://github.com/fairfieldai";

/** The towns the community draws from, roughly 15 minutes' drive from Fairfield. */
export const TOWNS = ["Fairfield", "Southport", "Westport", "Easton", "Trumbull"];

const CONNECTICUT = { "@type": "State", name: "Connecticut" };
// Southport is a village within Fairfield rather than a town of its own.
const AREA_SERVED = TOWNS.map((name) =>
  name === "Southport"
    ? { "@type": "Place", name, containedInPlace: { "@type": "City", name: "Fairfield" } }
    : { "@type": "City", name, containedInPlace: CONNECTICUT },
);

/** schema.org description of the community and the site, for search engines and agents. */
export const STRUCTURED_DATA = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Organization",
      "@id": `${SITE_URL}/#organization`,
      name: SITE_NAME,
      url: `${SITE_URL}/`,
      logo: `${SITE_URL}/icon-512.png`,
      email: CONTACT_EMAIL,
      description: SITE_DESCRIPTION,
      areaServed: AREA_SERVED,
      knowsAbout: ["Artificial intelligence", "Machine learning", "Large language models"],
      sameAs: [DISCORD_INVITE_URL, GITHUB_URL],
    },
    {
      "@type": "WebSite",
      "@id": `${SITE_URL}/#website`,
      name: SITE_NAME,
      url: `${SITE_URL}/`,
      description: SITE_DESCRIPTION,
      inLanguage: "en-US",
      publisher: { "@id": `${SITE_URL}/#organization` },
    },
  ],
};

const HOME_TITLE = `${SITE_NAME} · A local AI community for Fairfield, CT`;
const IMAGE = {
  url: "/opengraph-image.png",
  width: 1200,
  height: 630,
  alt: "fairfieldct.ai: A local AI community for Fairfield, Connecticut. A sun rising over Long Island Sound beneath a constellation of connected points.",
};

/**
 * Complete metadata for a page. Next.js replaces nested objects like `openGraph`
 * instead of merging them, so every page sets all of them through here.
 */
export function pageMetadata({
  path,
  title,
  description = SITE_DESCRIPTION,
}: {
  path: string;
  title?: string;
  description?: string;
}): Metadata {
  const fullTitle = title ? `${title} · ${SITE_NAME}` : HOME_TITLE;
  return {
    title: title ?? { absolute: HOME_TITLE },
    description,
    alternates: { canonical: path },
    openGraph: {
      type: "website",
      siteName: SITE_NAME,
      locale: "en_US",
      url: path,
      title: fullTitle,
      description,
      images: [IMAGE],
    },
    twitter: { card: "summary_large_image", title: fullTitle, description, images: [IMAGE] },
  };
}
