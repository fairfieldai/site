import type { Metadata, Viewport } from "next";
import { DM_Serif_Display, Geist, Geist_Mono } from "next/font/google";
import Link from "next/link";
import type { ReactNode } from "react";

import { DISCORD_INVITE_URL } from "@/lib/links";

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
  title: "fairfieldct.ai · A local AI community for Fairfield, CT",
  description:
    "A local community in Fairfield, Connecticut for builders, thinkers, and the AI-curious to learn, share, and build with AI together.",
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
