import { t } from "./i18n";

/** The folder received files are saved to, with a button to choose another. Used by Settings and
 * by first run. */
export function SaveFolderField({
  folder,
  disabled,
  onChange,
}: {
  /** The folder shown; null until it has been read. */
  folder: string | null;
  disabled?: boolean;
  /** The user wants to choose another folder. */
  onChange: () => void;
}) {
  return (
    <p>
      <span id="save-folder-label">{t("saveFolder.label")}: </span>
      <code aria-labelledby="save-folder-label">{folder ?? ""}</code>{" "}
      <button
        type="button"
        disabled={disabled || folder === null}
        aria-label={t("saveFolder.changeLabel")}
        onClick={onChange}
      >
        {t("saveFolder.change")}
      </button>
    </p>
  );
}
