import { useState } from "react";
import type { Api, Contact } from "./api";
import { peerName } from "./contacts";
import { t, type MessageKey } from "./i18n";
import { PlainText } from "./PlainText";
import {
  adjustedNamesText,
  batchStatus,
  canCancel,
  canResend,
  canRetry,
  formatSize,
  percent,
  transferName,
  type BatchView,
  type ListItem,
  type TransferView,
} from "./transfers";

/** Sentence for a Transfer's current state, e.g. "Waiting for Mum…" (or a Fingerprint). */
function statusText(x: TransferView, peer: string): string {
  const side = x.role === "sender" ? "sending" : "receiving";
  const params = {
    peer,
    reason: x.state.kind === "failed" ? x.state.reason : "",
  };
  if (x.state.kind === "cancelled") {
    return t(x.state.by === x.role ? "transfer.cancelledByYou" : "transfer.cancelledByPeer", params);
  }
  return t(`transfer.${side}.${x.state.kind}` as MessageKey, params);
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * The Transfers of this session, newest first: what is happening and how it ended. A Batch is
 * one row that opens to show a row for each Receiver.
 */
export function TransferList({
  api,
  contacts,
  items,
}: {
  api: Api;
  contacts: Contact[];
  items: ListItem[];
}) {
  return (
    <ul className="transfers">
      {items.map((item) =>
        item.kind === "batch" ? (
          <BatchRow key={item.batch.id} api={api} contacts={contacts} batch={item.batch} />
        ) : (
          <TransferRow key={item.transfer.id} api={api} contacts={contacts} transfer={item.transfer} />
        ),
      )}
    </ul>
  );
}

/** What one send to several Devices looks like: how many arrived, and each Device on request. */
function BatchRow({ api, contacts, batch }: { api: Api; contacts: Contact[]; batch: BatchView }) {
  const [open, setOpen] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const name = transferName(batch.members[0]);
  const list = `batch-${batch.id}`;

  return (
    <li>
      <strong>{t("batch.title", { name, count: batch.members.length })}</strong>
      {/* Announced as Devices finish, decline or fail. */}
      <p aria-live="polite">{batchStatus(batch)}</p>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={list}
        aria-label={`${t(open ? "batch.hide" : "batch.show")}: ${t("batch.toggleLabel", { name })}`}
        onClick={() => setOpen(!open)}
      >
        {t(open ? "batch.hide" : "batch.show")}
      </button>
      {batch.members.some(canCancel) && (
        <button
          type="button"
          aria-label={t("batch.cancelAllLabel", { name })}
          onClick={() => {
            setProblem(null);
            api.cancelBatch(batch.id).catch(() => setProblem(t("batch.cancelFailed")));
          }}
        >
          {t("batch.cancelAll")}
        </button>
      )}
      {problem !== null && <p role="alert">{problem}</p>}
      {open && (
        <ul id={list} className="members">
          {batch.members.map((x) => (
            <TransferRow key={x.id} api={api} contacts={contacts} transfer={x} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** A text that arrived: shown as plain text, with a button to copy it. */
function ReceivedText({ api, text, peer }: { api: Api; text: string; peer: string }) {
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  return (
    <>
      <PlainText text={text} />
      <button
        type="button"
        aria-label={t("transfer.copyLabel", { peer })}
        onClick={() =>
          api.copyText(text).then(
            () => setCopy("copied"),
            () => setCopy("failed"),
          )
        }
      >
        {t("transfer.copy")}
      </button>
      <span role="status" className="note">
        {copy === "copied" && t("transfer.copied")}
        {copy === "failed" && t("transfer.copyFailed")}
      </span>
    </>
  );
}

function TransferRow({
  api,
  contacts,
  transfer: x,
}: {
  api: Api;
  contacts: Contact[];
  transfer: TransferView;
}) {
  const [showFailed, setShowFailed] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // An expired Offer can be sent again once; the new Offer is a row of its own.
  const [resent, setResent] = useState(false);
  const peer = peerName(x.peer, contacts, x.peerName);
  const name = transferName(x);
  const title = t(x.role === "sender" ? "transfer.to" : "transfer.from", { name, peer });
  const moving =
    x.state.kind === "transferring" ||
    x.state.kind === "reconnecting" ||
    (x.state.kind === "accepted" && x.bytes > 0);
  const savedTo = x.state.kind === "completed" ? x.state.saved_to : null;

  return (
    <li>
      <strong>{title}</strong>
      {/* Announced when the state changes; the progress below is not, so it stays quiet. */}
      <p aria-live="polite">{statusText(x, peer)}</p>
      {x.role === "sender" && x.skippedLinks > 0 && (
        <p>
          {x.skippedLinks === 1
            ? t("transfer.linkSkipped")
            : t("transfer.linksSkipped", { count: x.skippedLinks })}
        </p>
      )}
      {x.role === "receiver" && x.adjustedNames > 0 && <p>{adjustedNamesText(x.adjustedNames)}</p>}
      {moving && (
        <p>
          <progress
            value={x.bytes}
            max={Math.max(x.size, 1)}
            aria-label={t("transfer.progressLabel", { name })}
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
            aria-label={t("transfer.showInFolderLabel", { name })}
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
      {x.role === "receiver" && x.kind === "text" && x.state.kind === "completed" && (
        <ReceivedText api={api} text={x.text ?? ""} peer={peer} />
      )}
      {canCancel(x) && (
        <button
          type="button"
          aria-label={t("transfer.cancelLabel", { name })}
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
          aria-label={t("transfer.resendLabel", { name })}
          onClick={() => {
            setProblem(null);
            api.resendTransfer(x.id).then(
              () => setResent(true),
              (e) => setProblem(t("transfer.resendFailed", { reason: errorText(e) })),
            );
          }}
        >
          {t("transfer.resend")}
        </button>
      )}
      {canRetry(x) && (
        <button
          type="button"
          aria-label={t("transfer.retryLabel", { name, peer })}
          onClick={() => {
            setProblem(null);
            // The new Offer is a Transfer of its own, which takes this Failed one's place
            // in the Batch.
            api.retryTransfer(x.id).catch((e) => setProblem(t("transfer.retryFailed", { reason: errorText(e) })));
          }}
        >
          {t("transfer.retry")}
        </button>
      )}
      {problem !== null && <p role="alert">{problem}</p>}
    </li>
  );
}
