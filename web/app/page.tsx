import type { Metadata } from "next";

import { STRUCTURED_DATA, TOWNS, pageMetadata } from "@/lib/site";

import { Account } from "./components/account";
import { Horizon } from "./components/horizon";
import { NextMeetup } from "./components/next-meetup";

const plans = [
  {
    title: "Meetups",
    body: "Relaxed evenings to talk about AI, technology, and our town. No expertise required.",
  },
  {
    title: "Demo nights",
    body: "Neighbors show what they're building, how they built it, and what they learned along the way.",
  },
  {
    title: "Learning together",
    body: "Hands-on sessions for putting AI tools to work, at the office, in the classroom, and at home.",
  },
];

export const metadata: Metadata = pageMetadata({ path: "/" });

export default function Home() {
  return (
    <>
      <script
        type="application/ld+json"
        // JSON.stringify of a constant; "<" is escaped so the data can't close the script tag.
        dangerouslySetInnerHTML={{
          __html: JSON.stringify(STRUCTURED_DATA).replace(/</g, "\\u003c"),
        }}
      />
      <main className="hero hero-home">
        <div className="hero-copy">
          <p className="eyebrow">{TOWNS.join(" · ")}</p>
          <h1>
            Neighbors figuring out AI <em>together</em>.
          </h1>
          <p className="lede">
            A friendly, local group for anyone curious about AI: parents, teachers, small-business
            owners, retirees, and people who build with it every day. Come learn from each other and
            talk through what this technology means for where we live. No expertise required.
          </p>
          <Account />
        </div>
        <Horizon />
      </main>

      <NextMeetup />

      <section className="plans" aria-labelledby="plans-heading">
        <h2 id="plans-heading">What we have in mind</h2>
        <ul>
          {plans.map((plan) => (
            <li key={plan.title}>
              <h3>{plan.title}</h3>
              <p>{plan.body}</p>
            </li>
          ))}
        </ul>
      </section>
    </>
  );
}
