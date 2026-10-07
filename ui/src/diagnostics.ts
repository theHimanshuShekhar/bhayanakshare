// Settings → Diagnostics: what the save dialog suggests for the zip.

/** `bhayanakshare-diagnostics-2026-10-07.zip`, for the day it is exported (the user's own). */
export function diagnosticsFileName(day: Date): string {
  const two = (n: number) => String(n).padStart(2, "0");
  return `bhayanakshare-diagnostics-${day.getFullYear()}-${two(day.getMonth() + 1)}-${two(day.getDate())}.zip`;
}
