import jsQR from "jsqr";
import { useEffect, useRef, useState } from "react";
import { t } from "./i18n";

/** How often a frame from the camera is looked at, and the widest it is looked at. */
const SCAN_EVERY_MS = 200;
const SCAN_WIDTH = 640;

/**
 * The webcam, looking for a QR code. `onDecoded` gets the text of every code it reads, as often
 * as it reads it; deciding whether that text is any use is the caller's job. The camera is
 * released when this goes away.
 */
export function QrScanner({ onDecoded }: { onDecoded: (text: string) => void }) {
  const video = useRef<HTMLVideoElement>(null);
  const decoded = useRef(onDecoded);
  decoded.current = onDecoded;
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    let stream: MediaStream | undefined;
    let timer: ReturnType<typeof setInterval> | undefined;
    const canvas = document.createElement("canvas");

    const look = () => {
      const view = video.current;
      if (!view || view.videoWidth === 0) return;
      const scale = Math.min(1, SCAN_WIDTH / view.videoWidth);
      canvas.width = Math.round(view.videoWidth * scale);
      canvas.height = Math.round(view.videoHeight * scale);
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) return;
      context.drawImage(view, 0, 0, canvas.width, canvas.height);
      const { data, width, height } = context.getImageData(0, 0, canvas.width, canvas.height);
      const code = jsQR(data, width, height, { inversionAttempts: "dontInvert" });
      if (code !== null && code.data !== "") decoded.current(code.data);
    };

    if (!navigator.mediaDevices?.getUserMedia) {
      setError(t("scan.unavailable"));
      return;
    }
    navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" } }).then(
      (opened) => {
        if (!live) {
          opened.getTracks().forEach((track) => track.stop());
          return;
        }
        stream = opened;
        if (video.current) {
          video.current.srcObject = opened;
          video.current.play().catch(() => {});
        }
        timer = setInterval(look, SCAN_EVERY_MS);
      },
      (e: unknown) => {
        const refused = e instanceof DOMException && e.name === "NotAllowedError";
        if (live) setError(t(refused ? "scan.refused" : "scan.unavailable"));
      },
    );

    return () => {
      live = false;
      clearInterval(timer);
      stream?.getTracks().forEach((track) => track.stop());
    };
  }, []);

  if (error !== null) return <p role="alert">{error}</p>;
  return (
    <>
      <video ref={video} className="scanner" muted playsInline aria-label={t("scan.video")} />
      <p role="status" className="note">
        {t("scan.hint")}
      </p>
    </>
  );
}
