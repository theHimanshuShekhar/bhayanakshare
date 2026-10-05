import { encode } from "uqr";

/**
 * `text` as a QR code, drawn inline (the page's content policy allows no images from data:
 * URLs). Always black on white with the full quiet zone, whatever the theme: a code that is
 * light on dark does not scan.
 */
export function QrCode({ text, label }: { text: string; label: string }) {
  const border = 4;
  const { data, size } = encode(text, { ecc: "M", border: 0 });
  const dark: string[] = [];
  data.forEach((row, y) =>
    row.forEach((on, x) => on && dark.push(`M${x + border} ${y + border}h1v1h-1z`)),
  );
  const side = size + 2 * border;
  return (
    <svg
      className="qr"
      role="img"
      aria-label={label}
      viewBox={`0 0 ${side} ${side}`}
      shapeRendering="crispEdges"
    >
      <rect width={side} height={side} fill="#fff" />
      <path d={dark.join("")} fill="#000" />
    </svg>
  );
}
