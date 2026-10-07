import type { Metadata } from "next";

import { STRUCTURED_DATA, pageMetadata } from "@/lib/site";

import { Account } from "./components/account";
import { Horizon } from "./components/horizon";

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
          <p className="eyebrow">Coming soon</p>
          <h1>
            A local AI community for <em>Fairfield</em>.
          </h1>
          <p className="lede">
            A place for builders, thinkers, and the AI-curious from across town to learn from each
            other, share what they&apos;re making, and talk through what this technology means for
            where we live. Technical or not, you&apos;re welcome here.
          </p>
          <Account />
        </div>
        <Horizon />
      </main>

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
