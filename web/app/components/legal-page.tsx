import type { ReactNode } from "react";

export function LegalPage({
  title,
  updated,
  children,
}: {
  title: string;
  updated: string;
  children: ReactNode;
}) {
  return (
    <main className="legal">
      <p className="eyebrow">Last updated {updated}</p>
      <h1>{title}</h1>
      {children}
    </main>
  );
}
