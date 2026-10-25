import type { AriaRole, ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

export type BannerKind = "info" | "warn" | "err" | "ok";

const ICONS: Record<BannerKind, IconName> = { info: "info", warn: "warn", err: "err", ok: "ok" };

/**
 * A banner: a notice across the page or a sheet, with its icon, an optional bold title, what it
 * says and the buttons it offers. Its kind is told by icon and border as well as colour. `role`
 * is left to the caller: whether it is news (`status`, `alert`) or a standing state (none)
 * depends on where it is shown.
 */
export function Banner({
  kind,
  title,
  children,
  actions,
  role,
  id,
}: {
  kind: BannerKind;
  title?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
  role?: AriaRole;
  id?: string;
}) {
  return (
    <div className={`bs-banner bs-banner--${kind}`} role={role} id={id}>
      <Icon name={ICONS[kind]} size={20} />
      <div>
        {title !== undefined && <p className="bs-banner__t">{title}</p>}
        {children}
      </div>
      {actions !== undefined && <div className="bs-banner__a">{actions}</div>}
    </div>
  );
}

/** A quiet line of help or state under a control or a list, with an optional icon. */
export function Hint({
  icon,
  children,
  role,
  id,
  tone,
}: {
  icon?: IconName;
  children: ReactNode;
  role?: AriaRole;
  id?: string;
  /** `err` for a failure said in place, `ok` for something done. */
  tone?: "err" | "ok";
}) {
  return (
    <p className={tone === undefined ? "bs-hint" : `bs-hint bs-hint--${tone}`} role={role} id={id}>
      {icon !== undefined && <Icon name={icon} />}
      <span>{children}</span>
    </p>
  );
}

/** What is wrong with a field, under it: an icon and one sentence, announced when it appears. */
export function FieldError({ id, children }: { id?: string; children: ReactNode }) {
  return (
    <p className="bs-msg" role="alert" id={id}>
      <Icon name="err" />
      <span>{children}</span>
    </p>
  );
}
