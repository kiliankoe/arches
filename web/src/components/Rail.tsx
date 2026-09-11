/**
 * The panel over the map: a left rail on a wide screen, a bottom sheet below 720 px. Which one it
 * is, is entirely a matter of CSS; the content is the same either way.
 */

import type { ReactNode } from "react";

export default function Rail({
  children,
  footer,
  wide,
}: {
  children: ReactNode;
  footer?: ReactNode;
  /** The month grid needs the width, so it takes the panel rather than the rail. */
  wide?: boolean;
}) {
  return (
    <aside className={wide ? "rail rail-wide" : "rail"}>
      <div className="rail-body">{children}</div>
      {footer ? <div className="rail-footer">{footer}</div> : null}
    </aside>
  );
}
