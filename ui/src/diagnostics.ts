// Settings → Diagnostics: what the save dialog suggests for the zip, and where to report a problem.

/** The bug-report issue form (`.github/ISSUE_TEMPLATE/bug_report.yml`), opened in the browser. */
export const BUG_REPORT_URL =
  "https://github.com/theHimanshuShekhar/bhayanakshare/issues/new?template=bug_report.yml";

/** `bhayanakshare-diagnostics-2026-10-07.zip`, for the day it is exported (the user's own). */
export function diagnosticsFileName(day: Date): string {
  const two = (n: number) => String(n).padStart(2, "0");
  return `bhayanakshare-diagnostics-${day.getFullYear()}-${two(day.getMonth() + 1)}-${two(day.getDate())}.zip`;
}
