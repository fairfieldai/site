import Link from "next/link";

export default function NotFound() {
  return (
    <main className="hero">
      <p className="eyebrow">404</p>
      <h1>This page washed out to sea.</h1>
      <p className="lede">
        <Link href="/">Head back to the home page</Link>
      </p>
    </main>
  );
}
