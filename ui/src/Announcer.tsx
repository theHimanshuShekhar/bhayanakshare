import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { announcements, type ProgressMemory } from "./announce";
import type { Contact } from "./api";
import { peerName } from "./contacts";
import type { Transfers } from "./transfers";

/** How long a line stays in the region. Long enough to be read out, short enough not to pile up. */
export const ANNOUNCEMENT_MS = 10_000;

const Announce = createContext<(message: string) => void>(() => {});

/** Says `message` to a screen reader, politely: after what it is saying now. A no-op where there is no Announcer. */
export const useAnnounce = () => useContext(Announce);

/**
 * The app's one live region. Everything that is said to a screen reader on its own (a Transfer
 * changing state, a copy having worked) goes through it, rather than each row and button keeping
 * a live region of its own: so what is said is in one order, nothing is said twice, and a state
 * that is shown in a row is not also read out because the row came into being.
 *
 * It is a `log`: lines are added, in order, and old ones go. Each message is a line of its own,
 * and only added lines are read (a `log` is not atomic, as a `status` is, which would read every
 * line again at each addition), so two lines said together are both read, and the same words said
 * twice are read twice, which one changing text would not do. It sits outside everything the app makes
 * inert, which a screen reader would skip. Errors that need the user to do something are not said
 * here but are `role="alert"` where they are shown.
 */
export function Announcer({ children }: { children: ReactNode }) {
  const [lines, setLines] = useState<{ id: number; text: string }[]>([]);
  const count = useRef(0);
  const timers = useRef(new Set<ReturnType<typeof setTimeout>>());

  const announce = useCallback((text: string) => {
    const id = count.current++;
    setLines((now) => [...now, { id, text }]);
    const timer = setTimeout(() => {
      timers.current.delete(timer);
      setLines((now) => now.filter((line) => line.id !== id));
    }, ANNOUNCEMENT_MS);
    timers.current.add(timer);
  }, []);

  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach(clearTimeout);
  }, []);

  return (
    <Announce value={announce}>
      {children}
      <div role="log" aria-live="polite" aria-relevant="additions" data-announcer className="visually-hidden">
        {lines.map((line) => (
          <p key={line.id}>{line.text}</p>
        ))}
      </div>
    </Announce>
  );
}

/** Says what the Transfers do, as `announcements` decides; shows nothing. Used inside an Announcer. */
export function TransferAnnouncements({ transfers, contacts }: { transfers: Transfers; contacts: Contact[] }) {
  const announce = useAnnounce();
  const previous = useRef(transfers);
  const memory = useRef<ProgressMemory>({});

  useEffect(() => {
    if (previous.current === transfers) return;
    const result = announcements(
      previous.current,
      transfers,
      memory.current,
      (view) => peerName(view.peer, contacts, view.peerName),
      Date.now(),
    );
    previous.current = transfers;
    memory.current = result.memory;
    result.messages.forEach(announce);
  }, [transfers, contacts, announce]);

  return null;
}
