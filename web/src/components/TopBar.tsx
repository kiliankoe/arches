/** The slim bar over the map: where you are, one step either way, and where else to look. */

import type { ReactNode } from "react";
import { Link } from "react-router";

type Props = {
  title: string;
  /** Where the previous and next step lead, or null at a dead end. */
  previous: { to: string; label: string } | null;
  next: { to: string; label: string } | null;
  children?: ReactNode;
};

export default function TopBar({ title, previous, next, children }: Props) {
  return (
    <header className="topbar">
      {previous || next ? (
        <div className="topbar-step">
          <Step side="previous" target={previous} />
          <Step side="next" target={next} />
        </div>
      ) : null}
      <h1 className="topbar-title">{title}</h1>
      <div className="topbar-controls">{children}</div>
    </header>
  );
}

function Step({ side, target }: { side: "previous" | "next"; target: Props["previous"] }) {
  const glyph = side === "previous" ? "‹" : "›";
  if (!target) {
    return (
      <span className="step step-dead" aria-hidden="true">
        {glyph}
      </span>
    );
  }
  return (
    <Link className="step" to={target.to} aria-label={target.label} title={target.label}>
      {glyph}
    </Link>
  );
}
