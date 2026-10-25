import { t } from "./i18n";

/** The folder received files are saved to, with a button to choose another: a label, then the path
 * and the button. It is the content of a row, which the caller makes. Used by Settings and
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
    <>
      <span id="save-folder-label" className="bs-label">
        {t("saveFolder.label")}
      </span>
      <div className="kv">
        <code className="bs-mono" aria-labelledby="save-folder-label">
          {folder ?? ""}
        </code>
        <button
          type="button"
         
          disabled={disabled || folder === null}
          aria-label={t("saveFolder.changeLabel")}
          onClick={onChange}
        >
          {t("saveFolder.change")}
        </button>
      </div>
    </>
  );
}
