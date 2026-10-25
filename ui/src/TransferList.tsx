import { useState } from "react";
import type { Api, Contact } from "./api";
import { peerName } from "./contacts";
import { Icon, type IconName } from "./Icon";
import { t } from "./i18n";
import { PlainText } from "./PlainText";
import { useCopy } from "./useCopy";
import {
  adjustedNamesText,
  batchStatus,
  canCancel,
  canResend,
  canRetry,
  formatSize,
  isOver,
  isPreparing,
  percent,
  skippedLinksText,
  statusText,
  transferName,
  type BatchView,
  type ListItem,
  type TransferView,
} from "./transfers";

export function errorText(e: unknown): string {
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
    <ul className="list">
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
    <li className={open ? "bs-row bs-row--on" : "bs-row"}>
      <div>
        <div className="bs-row__t">{t("batch.title", { name, count: batch.members.length })}</div>
        {/* Said once, when the Batch is over, by the Announcer: not a live region of its own. */}
        <p className="bs-row__m">{batchStatus(batch)}</p>
        {problem !== null && <p role="alert">{problem}</p>}
      </div>
      <div className="bs-btns">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={list}
          aria-label={`${t(open ? "batch.hide" : "batch.show")}: ${t("batch.toggleLabel", { name })}`}
          onClick={() => setOpen(!open)}
        >
          <Icon name={open ? "tri" : "trir"} />
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
      </div>
      {open && (
        <ul id={list} className="kids">
          {batch.members.map((x) => (
            <TransferRow key={x.id} api={api} contacts={contacts} transfer={x} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** A text, shown as plain text, with a button to copy it. `label` says which text, for the button. */
export function CopyableText({ api, text, label }: { api: Api; text: string; label: string }) {
  const { outcome, copy } = useCopy(api);
  return (
    <>
      <PlainText text={text} />
      <div className="bs-btns copy">
        <button type="button" aria-label={label} onClick={() => copy(text)}>
          {t("transfer.copy")}
        </button>
        {outcome === "copied" && (
          <span className="bs-row__s ok">
            <Icon name="ok" />
            {t("transfer.copied")}
          </span>
        )}
        {outcome === "failed" && (
          <span role="alert" className="bs-row__s">
            <Icon name="err" />
            {t("transfer.copyFailed")}
          </span>
        )}
      </div>
    </>
  );
}

/** The icon that goes with a Transfer's state line: its direction while it moves, else how it ended. */
function stateIcon(x: TransferView, moving: boolean): IconName {
  switch (x.state.kind) {
    case "reconnecting":
      return "warn";
    case "completed":
      return "ok";
    case "failed":
      return "err";
    case "declined":
    case "cancelled":
    case "expired":
      return "stop";
    case "transferring":
    case "saving":
      return x.role === "sender" ? "up" : "down";
    default:
      return moving ? (x.role === "sender" ? "up" : "down") : "clock";
  }
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
  const cancellable = canCancel(x);
  const resendable = canResend(x) && !resent;
  const retryable = canRetry(x);

  return (
    <li className={x.state.kind === "failed" ? "bs-row bs-row--err" : "bs-row"}>
      <div>
        <div className="bs-row__t">{title}</div>
        {/* The Announcer says when this changes, and the progress below at coarse steps. */}
        {x.state.kind === "failed" ? (
          <>
            <p className="bs-row__s">
              <Icon name="err" />
              <span>{t("transfer.failed")}</span>
            </p>
            <p className="bs-row__m bs-row__why">{statusText(x, peer)}</p>
          </>
        ) : (
          <p className={x.state.kind === "completed" ? "bs-row__s ok" : "bs-row__s"}>
            <Icon name={stateIcon(x, moving)} />
            <span>{statusText(x, peer, isPreparing(x))}</span>
          </p>
        )}
        {x.role === "sender" && x.skippedLinks > 0 && (
          <p className="bs-row__m">{skippedLinksText(x.skippedLinks)}</p>
        )}
        {x.role === "receiver" && x.adjustedNames > 0 && (
          <p className="bs-row__m">{adjustedNamesText(x.adjustedNames, isOver(x.state))}</p>
        )}
        {savedTo !== null && <p className="bs-row__m">{t("transfer.savedTo", { path: savedTo })}</p>}
        {showFailed && <p role="alert">{t("transfer.showInFolderFailed")}</p>}
        {x.role === "receiver" && x.kind === "text" && x.state.kind === "completed" && (
          <CopyableText api={api} text={x.text ?? ""} label={t("transfer.copyLabel", { peer })} />
        )}
        {problem !== null && <p role="alert">{problem}</p>}
      </div>
      {(savedTo !== null || cancellable || resendable || retryable) && (
        <div className="bs-btns">
          {savedTo !== null && (
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
          )}
          {cancellable && (
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
          {resendable && (
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
          {retryable && (
            <button
              type="button"
              aria-label={t("transfer.retryLabel", { name, peer })}
              onClick={() => {
                setProblem(null);
                // The new Offer is a Transfer of its own, which takes this Failed one's place
                // in the Batch.
                api
                  .retryTransfer(x.id)
                  .catch((e) => setProblem(t("transfer.retryFailed", { reason: errorText(e) })));
              }}
            >
              {t("transfer.retry")}
            </button>
          )}
        </div>
      )}
      {moving && (
        <div className="bs-row__p">
          <div className={x.state.kind === "reconnecting" ? "bs-prog bs-prog--re" : "bs-prog"}>
            <progress
              value={x.bytes}
              max={Math.max(x.size, 1)}
              aria-label={t("transfer.progressLabel", { name })}
            />
            <div className="bs-row__m">
              {t("transfer.progress", { percent: percent(x), size: formatSize(x.size) })}
              {x.rate !== null && ` · ${t("transfer.rate", { size: formatSize(x.rate) })}`}
            </div>
          </div>
        </div>
      )}
    </li>
  );
}
