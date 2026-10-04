import { useState } from "react";
import type { Api } from "./api";
import { t, type MessageKey } from "./i18n";
import {
  canCancel,
  canResend,
  fingerprint,
  formatSize,
  percent,
  type TransferView,
} from "./transfers";

/** Sentence for a Transfer's current state, e.g. "Waiting for K3QF-7XNA…". */
function statusText(x: TransferView): string {
  const side = x.role === "sender" ? "sending" : "receiving";
  const params = {
    peer: fingerprint(x.peer),
    reason: x.state.kind === "failed" ? x.state.reason : "",
  };
  if (x.state.kind === "cancelled") {
    return t(x.state.by === x.role ? "transfer.cancelledByYou" : "transfer.cancelledByPeer", params);
  }
  return t(`transfer.${side}.${x.state.kind}` as MessageKey, params);
}

/** The Transfers of this session, newest first: what is happening and how it ended. */
export function TransferList({ api, transfers }: { api: Api; transfers: TransferView[] }) {
  return (
    <ul className="transfers">
      {transfers.map((x) => (
        <TransferRow key={x.id} api={api} transfer={x} />
      ))}
    </ul>
  );
}

function TransferRow({ api, transfer: x }: { api: Api; transfer: TransferView }) {
  const [showFailed, setShowFailed] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // An expired Offer can be sent again once; the new Offer is a row of its own.
  const [resent, setResent] = useState(false);
  const peer = fingerprint(x.peer);
  const title = t(x.role === "sender" ? "transfer.to" : "transfer.from", { name: x.name, peer });
  const moving = x.state.kind === "transferring" || (x.state.kind === "accepted" && x.bytes > 0);
  const savedTo = x.state.kind === "completed" ? x.state.saved_to : null;

  return (
    <li>
      <strong>{title}</strong>
      {/* Announced when the state changes; the progress below is not, so it stays quiet. */}
      <p aria-live="polite">{statusText(x)}</p>
      {moving && (
        <p>
          <progress
            value={x.bytes}
            max={Math.max(x.size, 1)}
            aria-label={t("transfer.progressLabel", { name: x.name })}
          />{" "}
          {t("transfer.progress", { percent: percent(x), size: formatSize(x.size) })}
          {x.rate !== null && ` · ${t("transfer.rate", { size: formatSize(x.rate) })}`}
        </p>
      )}
      {savedTo !== null && (
        <p>
          {t("transfer.savedTo", { path: savedTo })}{" "}
          <button
            type="button"
            aria-label={t("transfer.showInFolderLabel", { name: x.name })}
            onClick={() => {
              setShowFailed(false);
              api.showInFolder(savedTo).catch(() => setShowFailed(true));
            }}
          >
            {t("transfer.showInFolder")}
          </button>
        </p>
      )}
      {showFailed && <p role="alert">{t("transfer.showInFolderFailed")}</p>}
      {canCancel(x) && (
        <button
          type="button"
          aria-label={t("transfer.cancelLabel", { name: x.name })}
          onClick={() => {
            setProblem(null);
            api.cancelTransfer(x.id).catch(() => setProblem(t("transfer.cancelFailed")));
          }}
        >
          {t("transfer.cancel")}
        </button>
      )}
      {canResend(x) && !resent && (
        <button
          type="button"
          aria-label={t("transfer.resendLabel", { name: x.name })}
          onClick={() => {
            setProblem(null);
            api.resendTransfer(x.id).then(
              () => setResent(true),
              (e) =>
                setProblem(
                  t("transfer.resendFailed", { reason: e instanceof Error ? e.message : String(e) }),
                ),
            );
          }}
        >
          {t("transfer.resend")}
        </button>
      )}
      {problem !== null && <p role="alert">{problem}</p>}
    </li>
  );
}
