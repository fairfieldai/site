import type { Metadata } from "next";
import Link from "next/link";

import { LegalPage } from "../components/legal-page";

export const metadata: Metadata = {
  title: "Terms of Service · fairfieldct.ai",
};

export default function Terms() {
  return (
    <LegalPage title="Terms of Service" updated="October 7, 2026">
      <p className="lede">
        fairfieldct.ai is a volunteer-run community for people in and around Fairfield, Connecticut
        who are interested in artificial intelligence. These terms apply when you use
        fairfieldct.ai, create an account, email us, come to our events, or join our Discord server.
        By doing any of those, you agree to them.
      </p>

      <h2>Who can join</h2>
      <p>
        You must be at least 13 years old. Our Discord server also requires you to meet{" "}
        <a href="https://discord.com/terms">Discord&apos;s Terms of Service</a>.
      </p>

      <h2>Your account</h2>
      <p>
        Use an email address you control and keep your sign-in details to yourself. You&apos;re
        responsible for what happens under your account. You can ask us to delete it at any time.
      </p>

      <h2>Community guidelines</h2>
      <p>
        We want a welcoming place for builders and beginners, technical or not. Online and in
        person:
      </p>
      <ul>
        <li>Be respectful. No harassment, hate speech, threats, or personal attacks.</li>
        <li>No spam, unsolicited advertising, or scams.</li>
        <li>
          Don&apos;t share other people&apos;s personal information or content you don&apos;t have
          the right to share.
        </li>
        <li>Don&apos;t post anything illegal or sexually explicit.</li>
        <li>
          When sharing AI-generated content, don&apos;t present it as someone else&apos;s words or
          use it to deceive.
        </li>
      </ul>
      <p>
        Organizers may remove content, and suspend or remove anyone who breaks these guidelines,
        from the site, events, or Discord.
      </p>

      <h2>What you share</h2>
      <p>
        You keep ownership of what you post, present, or send us. By sharing it in a community
        space, you let other members see it and let us display it there. We won&apos;t use your
        projects or talks to promote the community without asking first.
      </p>

      <h2>Events</h2>
      <p>
        Events are run by volunteers, often in venues we don&apos;t control, and you attend at your
        own risk. Talks, demos, and discussions share the speakers&apos; own views and aren&apos;t
        professional, legal, financial, or medical advice. Events may be photographed; let an
        organizer know if you&apos;d rather not appear in photos.
      </p>

      <h2>No warranties</h2>
      <p>
        We provide the site, our services, and our events as they are, without warranties of any
        kind. They may change, be interrupted, or end.
      </p>

      <h2>Limitation of liability</h2>
      <p>
        To the fullest extent the law allows, fairfieldct.ai and its volunteers aren&apos;t liable
        for indirect, incidental, or consequential damages, or for any loss arising from your use of
        the site, our services, or our events.
      </p>

      <h2>Changes</h2>
      <p>
        We may update these terms. We&apos;ll change the date at the top of this page, and for
        significant changes we&apos;ll let members know by email or on Discord. Continuing to take
        part after a change means you accept the new terms.
      </p>

      <h2>Governing law</h2>
      <p>These terms are governed by the laws of the State of Connecticut.</p>

      <h2>Contact</h2>
      <p>
        Questions? Email <a href="mailto:hello@inbox.fairfieldct.ai">hello@inbox.fairfieldct.ai</a>.
        See also our <Link href="/privacy/">Privacy Policy</Link>.
      </p>
    </LegalPage>
  );
}
