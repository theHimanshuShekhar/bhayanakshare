import { useEffect, useState } from "react";
import type { Api, BatchId, Contact, HistoryEntry, Role, TransferId } from "./api";
import type { HistoryTransfer } from "./bindings";
import { Banner, Hint } from "./Banner";
import { peerName } from "./contacts";
import {
  canRetry,
  deviceOptions,
  formatTime,
  latestPerReceiver,
  noteDevices,
  retriedIds,
  type KnownDevices,
} from "./history";
import { Icon, type IconName } from "./Icon";
import { t } from "./i18n";
import { errorText } from "./TransferList";
import { adjustedNamesText, batchStatus, formatSize, isOver, statusText, transferName } from "./transfers";
import { useCopy } from "./useCopy";

/**
 * The History tab: every Transfer this Device has sent or received, newest first, that can be
 * narrowed by Device and direction and searched by item name. A Batch this Device sent is one
 * entry that opens to show each Receiver; with a Device chosen, it is that Receiver's Transfer.
 */
export function HistoryScreen({
  api,
  contacts,
  device,
  onDevice,
  stamp,
  version,
  onClear,
}: {
  api: Api;
  contacts: Contact[];
  /** The Device History is narrowed to; null for any. */
  device: string | null;
  onDevice: (device: string | null) => void;
  /** Changes whenever a Transfer of this session changes state, so History is read again. */
  stamp: string;
  /** Changes when History was cleared. */
  version: number;
  /** Ask whether to clear History. */
  onClear: () => void;
}) {
  const [direction, setDirection] = useState<Role | "">("");
  const [search, setSearch] = useState("");
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  const [failed, setFailed] = useState(false);
  // The Devices seen so far, to choose among: narrowing to one must not forget the others.
  const [known, setKnown] = useState<KnownDevices>({});
  // Bumped when an entry was deleted, to read History again.
  const [deleted, setDeleted] = useState(0);

  useEffect(() => {
    let live = true;
    api.history(device, direction || null, search.trim() || null).then(
      (found) => {
        if (!live) return;
        setEntries(found);
        setFailed(false);
        setKnown((now) => noteDevices(now, found));
      },
      () => live && setFailed(true),
    );
    return () => {
      live = false;
    };
  }, [api, device, direction, search, stamp, version, deleted]);

  const options = deviceOptions(contacts, known);
  const retried = retriedIds(entries ?? []);
  const narrowed = device !== null || direction !== "" || search.trim() !== "";
  const changed = () => setDeleted((n) => n + 1);

  return (
    <section aria-labelledby="history-heading" className="sect">
      <div className="pagehead">
        <h2 id="history-heading" className="bs-h1">
          {t("history.heading")}
        </h2>
        <button type="button" onClick={onClear}>
          {t("history.clear")}
        </button>
      </div>
      <form role="search" aria-label={t("history.filters")} className="filters" onSubmit={(e) => e.preventDefault()}>
        <div className="bs-field">
          <label className="bs-label" htmlFor="history-device">
            {t("history.deviceLabel")}
          </label>
          <span className="bs-selwrap">
            <select
              id="history-device"
              className="bs-input bs-select"
              value={device ?? ""}
              onChange={(e) => onDevice(e.target.value || null)}
            >
              <option value="">{t("history.anyDevice")}</option>
              {options.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
            <Icon name="tri" size={14} />
          </span>
        </div>
        <div className="bs-field">
          <label className="bs-label" htmlFor="history-direction">
            {t("history.directionLabel")}
          </label>
          <span className="bs-selwrap">
            <select
              id="history-direction"
              className="bs-input bs-select"
              value={direction}
              onChange={(e) => setDirection(e.target.value as Role | "")}
            >
              <option value="">{t("history.anyDirection")}</option>
              <option value="sender">{t("history.sent")}</option>
              <option value="receiver">{t("history.received")}</option>
            </select>
            <Icon name="tri" size={14} />
          </span>
        </div>
        <div className="bs-field filters__q">
          <label className="bs-label" htmlFor="history-search">
            {t("history.searchLabel")}
          </label>
          <input
            id="history-search"
            className="bs-input"
            type="search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            autoComplete="off"
          />
        </div>
      </form>
      {failed && (
        <Banner kind="err" role="alert">
          {t("history.loadFailed")}
        </Banner>
      )}
      {entries !== null && entries.length === 0 && (
        <p className="empty">{t(narrowed ? "history.noMatch" : "history.empty")}</p>
      )}
      {entries !== null && entries.length > 0 && (
        <ul className="list">
          {entries.map((entry) =>
            entry.kind === "batch" ? (
              <HistoryBatch
                key={entry.batch_id}
                api={api}
                contacts={contacts}
                batchId={entry.batch_id}
                transfers={entry.transfers}
                retried={retried}
                onDeleted={changed}
              />
            ) : (
              <HistoryRow
                key={entry.transfer.record.id}
                api={api}
                contacts={contacts}
                transfer={entry.transfer}
                retried={retried}
                onDeleted={changed}
              />
            ),
          )}
        </ul>
      )}
    </section>
  );
}

/** A Batch this Device sent, as one entry: how many arrived, and each Receiver on request. */
function HistoryBatch({
  api,
  contacts,
  batchId,
  transfers,
  retried,
  onDeleted,
}: {
  api: Api;
  contacts: Contact[];
  batchId: BatchId;
  transfers: HistoryTransfer[];
  retried: Set<TransferId>;
  onDeleted: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const receivers = latestPerReceiver(transfers);
  const name = transferName(transfers[0].record);
  const list = `history-batch-${batchId}`;

  return (
    <li className={open ? "bs-row bs-row--on" : "bs-row"}>
      <div>
        <div className="bs-row__t">{t("batch.title", { name, count: receivers.length })}</div>
        <div className="hmeta bs-row__m">
          <span>{t("history.offered", { time: formatTime(transfers[0].record.created_at) })}</span>
          <span>{batchStatus({ members: receivers.map((r) => r.record) })}</span>
        </div>
        {problem !== null && (
          <Hint icon="err" tone="err" role="alert">
            {problem}
          </Hint>
        )}
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
        {transfers.every((x) => isOver(x.record.state)) && (
          <button
            type="button"
            aria-label={t("history.deleteAllLabel", { name })}
            onClick={() => {
              setProblem(null);
              api.deleteHistoryBatch(batchId).then(onDeleted, (e) =>
                setProblem(t("history.deleteFailed", { reason: errorText(e) })),
              );
            }}
          >
            {t("history.deleteAll")}
          </button>
        )}
      </div>
      {open && (
        <ul id={list} className="kids">
          {transfers.map((x) => (
            <HistoryRow
              key={x.record.id}
              api={api}
              contacts={contacts}
              transfer={x}
              retried={retried}
              onDeleted={onDeleted}
            />
          ))}
        </ul>
      )}
    </li>
  );
}

/** The icon beside how a Transfer stands: done, failed, stopped before it was done, or still going. */
function stateIcon(state: HistoryTransfer["record"]["state"]): IconName {
  switch (state.kind) {
    case "completed":
      return "ok";
    case "failed":
      return "err";
    case "declined":
    case "expired":
    case "cancelled":
      return "stop";
    default:
      return "clock";
  }
}

/** "Saved to <path>", with the path in the monospace face, from the words of the one message. */
function SavedTo({ path }: { path: string }) {
  const [before, after] = t("transfer.savedTo", { path: "\u0000" }).split("\u0000");
  return (
    <>
      {before}
      <span className="bs-mono">{path}</span>
      {after}
    </>
  );
}

/** One Transfer: whom with, what, when, how it ended, and what can be done with it now. */
function HistoryRow({
  api,
  contacts,
  transfer,
  retried,
  onDeleted,
}: {
  api: Api;
  contacts: Contact[];
  transfer: HistoryTransfer;
  retried: Set<TransferId>;
  onDeleted: () => void;
}) {
  const { record: x, saved_present: present } = transfer;
  const [problem, setProblem] = useState<string | null>(null);
  const [showFailed, setShowFailed] = useState(false);
  const { outcome, copy } = useCopy(api);
  const peer = peerName(x.peer, contacts, x.peer_name);
  const name = transferName(x);
  const title = t(x.role === "sender" ? "transfer.to" : "transfer.from", { name, peer });
  const savedTo = x.state.kind === "completed" ? x.state.saved_to : null;
  const text = x.kind === "text" ? x.text : null;
  const details = [formatSize(x.size)];
  if (x.kind === "files") {
    details.push(x.file_count === 1 ? t("history.fileOne") : t("history.files", { count: x.file_count }));
  }
  if (x.role === "receiver" && x.adjusted_names > 0) details.push(adjustedNamesText(x.adjusted_names, x.state.kind === "completed"));

  return (
    <li className="bs-row">
      <div>
        <div className="bs-row__t">{title}</div>
        {x.state.kind === "failed" ? (
          <>
            <div className="bs-row__s">
              <Icon name="err" />
              <span>{t("transfer.failed")}</span>
            </div>
            <p className="bs-row__why">{statusText(x, peer)}</p>
          </>
        ) : (
          <div className={x.state.kind === "completed" ? "bs-row__s ok" : "bs-row__s"}>
            <Icon name={stateIcon(x.state)} />
            <span>{statusText(x, peer)}</span>
          </div>
        )}
        <div className="hmeta bs-row__m">
          <span>{details.join(" · ")}</span>
          <span>
            {[
              t("history.offered", { time: formatTime(x.created_at) }),
              ...(x.accepted_at !== null ? [t("history.accepted", { time: formatTime(x.accepted_at) })] : []),
              ...(isOver(x.state) ? [t("history.finished", { time: formatTime(x.updated_at) })] : []),
            ].join(" · ")}
          </span>
        </div>
        {savedTo !== null && (
          <div className="hsaved">
            <SavedTo path={savedTo} />
          </div>
        )}
        {savedTo !== null && present === false && (
          <div className="gone">
            <Icon name="warn" />
            <span>{t("history.fileGone", { path: savedTo })}</span>
          </div>
        )}
        {text !== null && (
          <pre className="bs-textbox hsaved" tabIndex={0} role="region" aria-label={t("text.label")}>
            {text}
          </pre>
        )}
        {outcome === "copied" && (
          <Hint icon="ok" tone="ok">
            {t("transfer.copied")}
          </Hint>
        )}
        {outcome === "failed" && (
          <Hint icon="err" tone="err" role="alert">
            {t("transfer.copyFailed")}
          </Hint>
        )}
        {showFailed && (
          <Hint icon="err" tone="err" role="alert">
            {t("transfer.showInFolderFailed")}
          </Hint>
        )}
        {problem !== null && (
          <Hint icon="err" tone="err" role="alert">
            {problem}
          </Hint>
        )}
      </div>
      <div className="bs-btns">
        {savedTo !== null && present !== false && (
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
        {text !== null && (
          <button
            type="button"
            aria-label={t(x.role === "receiver" ? "transfer.copyLabel" : "history.copySentLabel", { peer })}
            onClick={() => copy(text)}
          >
            {t("transfer.copy")}
          </button>
        )}
        {canRetry(transfer, retried) && (
          <button
            type="button"
            aria-label={t("transfer.retryLabel", { name, peer })}
            onClick={() => {
              setProblem(null);
              api.retryTransfer(x.id).catch((e) => setProblem(t("transfer.retryFailed", { reason: errorText(e) })));
            }}
          >
            {t("transfer.retry")}
          </button>
        )}
        {isOver(x.state) && (
          <button
            type="button"
            aria-label={t("history.deleteLabel", { title })}
            onClick={() => {
              setProblem(null);
              api.deleteHistoryTransfer(x.id).then(onDeleted, (e) =>
                setProblem(t("history.deleteFailed", { reason: errorText(e) })),
              );
            }}
          >
            {t("history.delete")}
          </button>
        )}
      </div>
    </li>
  );
}
