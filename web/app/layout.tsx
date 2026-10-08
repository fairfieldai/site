import type { Metadata, Viewport } from "next";
import { DM_Serif_Display, Geist, Geist_Mono } from "next/font/google";
import Link from "next/link";
import type { ReactNode } from "react";

import { DISCORD_INVITE_URL } from "@/lib/links";
import { SITE_DESCRIPTION, SITE_NAME, SITE_URL } from "@/lib/site";

import { Shoreline } from "./components/shoreline";
import "./globals.css";

// next/font bundles these at build time, so the site makes no requests to
// Google Fonts.
const serif = DM_Serif_Display({
  weight: "400",
  style: ["normal", "italic"],
  subsets: ["latin"],
  variable: "--font-serif",
});
const sans = Geist({ subsets: ["latin"], variable: "--font-sans" });
const mono = Geist_Mono({ subsets: ["latin"], variable: "--font-mono" });

export const metadata: Metadata = {
  metadataBase: new URL(SITE_URL),
  title: {
    default: `${SITE_NAME} · A local AI community for Fairfield, CT`,
    template: `%s · ${SITE_NAME}`,
  },
  description: SITE_DESCRIPTION,
  applicationName: SITE_NAME,
  authors: [{ name: `${SITE_NAME} organizers`, url: "/humans.txt" }],
  keywords: [
    "Fairfield",
    "Connecticut",
    "AI",
    "artificial intelligence",
    "community",
    "meetups",
    "machine learning",
    "Fairfield County",
  ],
};

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#fbf7f0" },
    { media: "(prefers-color-scheme: dark)", color: "#0c1a25" },
  ],
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className={`${serif.variable} ${sans.variable} ${mono.variable}`}>
      <body>
        <div className="page">
          <header className="site-header">
            <Link href="/" className="wordmark">
              fairfieldct<span>.ai</span>
            </Link>
            <nav className="site-nav" aria-label="Main">
              <Link href="/events/">Meetups</Link>
              <Link href="/account/">Account</Link>
            </nav>
          </header>
          {children}
          <footer className="site-footer">
            <span className="coordinates">Fairfield, Connecticut · 41.14° N, 73.26° W</span>
            <nav className="footer-links" aria-label="Footer">
              <Link href="/terms/">Terms</Link>
              <Link href="/privacy/">Privacy</Link>
              <a href={DISCORD_INVITE_URL}>Discord</a>
              <a href="mailto:hello@inbox.fairfieldct.ai">hello@inbox.fairfieldct.ai</a>
            </nav>
          </footer>
        </div>
        <Shoreline />
      </body>
    </html>
  );
}
