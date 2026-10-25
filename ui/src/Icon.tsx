import type { ReactNode } from "react";

/** The design's icons, drawn on a 20 px grid in the text colour. */
const PATHS = {
  check: <path d="M4 10.5l4 4 8-9" fill="none" stroke="currentColor" strokeWidth="2.2" />,
  dot: <circle cx="10" cy="10" r="5" fill="currentColor" />,
  plus: <path d="M10 3v14M3 10h14" stroke="currentColor" strokeWidth="2" fill="none" />,
  warn: (
    <>
      <path d="M10 2.5L18.5 17.5H1.5z" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
      <path d="M10 8v4.5M10 14.6v.9" stroke="currentColor" strokeWidth="2" />
    </>
  ),
  err: (
    <>
      <circle cx="10" cy="10" r="8" fill="none" stroke="currentColor" strokeWidth="2" />
      <path d="M6.8 6.8l6.4 6.4M13.2 6.8l-6.4 6.4" stroke="currentColor" strokeWidth="2" />
    </>
  ),
  ok: (
    <>
      <circle cx="10" cy="10" r="8" fill="none" stroke="currentColor" strokeWidth="2" />
      <path d="M6 10.3l2.8 2.8L14 7.6" fill="none" stroke="currentColor" strokeWidth="2" />
    </>
  ),
  info: (
    <>
      <circle cx="10" cy="10" r="8" fill="none" stroke="currentColor" strokeWidth="2" />
      <path d="M10 9v5M10 5.8v.9" stroke="currentColor" strokeWidth="2" />
    </>
  ),
  stop: (
    <>
      <circle cx="10" cy="10" r="8" fill="none" stroke="currentColor" strokeWidth="2" />
      <path d="M6 10h8" stroke="currentColor" strokeWidth="2" />
    </>
  ),
  clock: (
    <>
      <circle cx="10" cy="10" r="8" fill="none" stroke="currentColor" strokeWidth="2" />
      <path d="M10 5.5V10l3 2" fill="none" stroke="currentColor" strokeWidth="2" />
    </>
  ),
  tri: <path d="M4.5 7.5h11L10 14z" fill="currentColor" />,
  trir: <path d="M7.5 4.5v11L14 10z" fill="currentColor" />,
  down: <path d="M10 3v12M5 10.5l5 5 5-5" fill="none" stroke="currentColor" strokeWidth="2" />,
  up: <path d="M10 17V5M5 9.5l5-5 5 5" fill="none" stroke="currentColor" strokeWidth="2" />,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof PATHS;

/** An icon beside words that say the same: hidden from screen readers, never the only sign. */
export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg
      className="bs-ic"
      width={size}
      height={size}
      style={{ width: size, height: size }}
      viewBox="0 0 20 20"
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[name]}
    </svg>
  );
}

/** The brand mark: a filled square (the file) in the accent and an open one (the Device it goes
 * to) in the text colour, so it follows the theme. Beside the name, which says it in words. */
export function BrandMark({ size = 24 }: { size?: number }) {
  return (
    <svg className="mark" width={size} height={size} viewBox="0 0 64 64" aria-hidden="true" focusable="false">
      <g transform="translate(-1 1)">
        <rect className="mark__file" x="9" y="27" width="28" height="28" rx="7" />
        <path
          className="mark__device"
          d="M27 23V16A7 7 0 0 1 34 9H48A7 7 0 0 1 55 16V30A7 7 0 0 1 48 37H41"
          fill="none"
          strokeWidth="4"
        />
      </g>
    </svg>
  );
}
