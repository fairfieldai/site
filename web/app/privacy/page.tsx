import type { Metadata } from "next";
import Link from "next/link";

import { LegalPage } from "../components/legal-page";

export const metadata: Metadata = {
  title: "Privacy Policy · fairfieldct.ai",
};

export default function Privacy() {
  return (
    <LegalPage title="Privacy Policy" updated="October 7, 2026">
      <p className="lede">
        fairfieldct.ai is a volunteer-run community for people in and around Fairfield, Connecticut
        who are interested in artificial intelligence. This policy explains what we collect when you
        use fairfieldct.ai, email us, or join our Discord server, and what we do with it. The short
        version: we collect very little, we don&apos;t sell it, and we don&apos;t track you across
        the web.
      </p>

      <h2>What we collect</h2>
      <h3>When you create an account</h3>
      <p>
        Accounts are managed by Amazon Cognito. We store your email address and the sign-in methods
        you set up: a password, which Cognito stores securely and we can never see, and any passkeys
        you register, of which we keep only the public part. Cognito also records when your account
        was created and when you last signed in.
      </p>
      <h3>When you email us</h3>
      <p>
        Messages sent to hello@inbox.fairfieldct.ai are stored so organizers can read and answer
        them, including your address, the subject, the message, and any attachments. Organizers may
        be notified of new messages in a private channel on our Discord server, with the sender,
        subject, and a short preview.
      </p>
      <h3>When we email you</h3>
      <p>
        Emails we send from our community inbox may include a small image that records when the
        message is opened, and links that pass through click.inbox.fairfieldct.ai to record when
        they&apos;re clicked. We use this to know whether our messages are reaching people. Sign-in
        codes and account verification emails aren&apos;t tracked.
      </p>
      <h3>When you visit the site</h3>
      <p>
        The site has no analytics, advertising, or tracking cookies. When you&apos;re signed in,
        your sign-in tokens are kept in your browser&apos;s session storage and cleared when you
        sign out or close the tab. Cognito&apos;s sign-in pages at auth.fairfieldct.ai use cookies
        to keep you signed in. Requests to our API, which powers features like your account, are
        logged with your IP address and browser details for troubleshooting and security.
      </p>
      <h3>On Discord</h3>
      <p>
        Our Discord server is run on Discord, and what you post there is governed by{" "}
        <a href="https://discord.com/privacy">Discord&apos;s Privacy Policy</a>. Our bot only
        manages the server and posts notifications to organizer channels. It doesn&apos;t collect or
        store your messages.
      </p>

      <h2>How we use it</h2>
      <ul>
        <li>To run your account and let you sign in.</li>
        <li>
          To reply to your messages and tell you about community events you&apos;ve signed up for.
        </li>
        <li>To keep the site, inbox, and Discord server secure and free of spam and abuse.</li>
      </ul>
      <p>We don&apos;t sell or rent your information, and we don&apos;t use it for advertising.</p>

      <h2>Who we share it with</h2>
      <p>
        We use a small number of services to run the community: Amazon Web Services, which hosts the
        site, accounts, and inbox in the United States, and Discord, which runs our community
        server. They process information on our behalf under their own terms. We share information
        with others only if the law requires it or to protect the safety of our members.
      </p>

      <h2>How long we keep it</h2>
      <ul>
        <li>Account information: until you ask us to delete your account.</li>
        <li>Emails to our inbox: up to one year.</li>
        <li>Email delivery, open, and click records: up to one year.</li>
        <li>API logs: 30 days.</li>
      </ul>

      <h2>Your choices</h2>
      <p>
        You can ask us to see, correct, or delete your information, including your account, at any
        time by emailing <a href="mailto:hello@inbox.fairfieldct.ai">hello@inbox.fairfieldct.ai</a>.
        You can block email tracking by turning off automatic image loading in your email app.
      </p>

      <h2>Children</h2>
      <p>
        fairfieldct.ai is for people 13 and older. We don&apos;t knowingly collect information from
        children under 13. If you believe a child has created an account, contact us and we&apos;ll
        delete it.
      </p>

      <h2>Changes</h2>
      <p>
        If we change this policy, we&apos;ll update the date at the top of this page, and for
        significant changes we&apos;ll let members know by email or on Discord.
      </p>

      <h2>Contact</h2>
      <p>
        Questions about privacy? Email{" "}
        <a href="mailto:hello@inbox.fairfieldct.ai">hello@inbox.fairfieldct.ai</a>. See also our{" "}
        <Link href="/terms/">Terms of Service</Link>.
      </p>
    </LegalPage>
  );
}
