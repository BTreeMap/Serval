import { useCallback, useEffect, useRef } from "react";
import { Button } from "./Button";
import { Banner } from "./feedback";
import * as Icons from "./icons";
import { useInlineEdit } from "../lib/useInlineEdit";

/** A seamless, Notion-style inline field. In display mode it reads as plain
 *  text — or a faint placeholder when empty — and reveals an edit affordance on
 *  hover. Clicking turns it into a borderless control that matches the
 *  displayed typography exactly (via `displayClass`), so editing feels in-place
 *  rather than form-like. Saving is always explicit: Enter (⌘/Ctrl+Enter for
 *  multiline) or the Save button; Escape cancels. Saving an empty value yields
 *  an empty string to `onSave`, leaving the clearing semantics to the caller.
 *
 *  The edit lifecycle lives in {@link useInlineEdit}, shared with the content-type
 *  editor; this component is the presentation of that state machine. */
export function InlineField({
  value,
  onSave,
  placeholder,
  ariaLabel,
  displayClass,
  multiline = false,
  rows = 3,
}: {
  value: string | null;
  onSave: (next: string) => Promise<void>;
  placeholder: string;
  ariaLabel: string;
  displayClass: string;
  multiline?: boolean;
  rows?: number;
}) {
  const initial = useCallback(() => value ?? "", [value]);
  // An empty value is a legitimate save here — it clears the field — so only an
  // unchanged value is a no-op.
  const isUnchanged = useCallback((next: string) => next === (value ?? ""), [value]);
  const edit = useInlineEdit({ initial, isUnchanged, commit: onSave });
  const isOpen = edit.isOpen;

  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement | null>(null);

  // On entering edit mode, focus the control and place the caret at the end.
  // Keyed on `isOpen`, which stays true across saving and failure, so a failed
  // save does not yank the caret back to the end of the text.
  useEffect(() => {
    if (!isOpen) {
      return;
    }
    const el = inputRef.current;
    if (!el) {
      return;
    }
    el.focus();
    const end = el.value.length;
    el.setSelectionRange(end, end);
  }, [isOpen]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      edit.cancel();
    } else if (event.key === "Enter" && (!multiline || event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      edit.save();
    }
  };

  if (!isOpen) {
    return (
      <button
        type="button"
        onClick={edit.begin}
        aria-label={`Edit ${ariaLabel}`}
        className={`group/field -mx-2 flex w-full items-start gap-2 rounded-md px-2 py-1 text-left transition-colors hover:bg-canvas focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/40 ${displayClass}`}
      >
        <span className={`min-w-0 flex-1 ${multiline ? "whitespace-pre-wrap break-words" : "truncate"}`}>
          {value ?? <span className="font-normal text-ink-faint">{placeholder}</span>}
        </span>
        <Icons.Pencil
          className="mt-1 h-3.5 w-3.5 shrink-0 text-ink-faint opacity-0 transition-opacity group-hover/field:opacity-100"
          aria-hidden
        />
      </button>
    );
  }

  const controlClass = `w-full rounded-md bg-canvas px-2 py-1 placeholder:text-ink-faint ring-1 ring-wisteria/40 transition focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/60 ${displayClass}`;

  return (
    <div className="-mx-2 space-y-2">
      {multiline ? (
        <textarea
          ref={(el) => {
            inputRef.current = el;
          }}
          value={edit.draft}
          onChange={(e) => edit.setDraft(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          rows={rows}
          aria-label={ariaLabel}
          className={`${controlClass} resize-none`}
        />
      ) : (
        <input
          ref={(el) => {
            inputRef.current = el;
          }}
          value={edit.draft}
          onChange={(e) => edit.setDraft(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          aria-label={ariaLabel}
          className={controlClass}
        />
      )}
      <div className="flex flex-wrap items-center gap-2 px-2">
        <Button size="sm" loading={edit.isSaving} onClick={edit.save}>
          {edit.isSaving ? "Saving…" : "Save"}
        </Button>
        <Button variant="ghost" size="sm" onClick={edit.cancel} disabled={edit.isSaving}>
          Cancel
        </Button>
        <span className="text-xs text-ink-faint">
          {multiline ? "⌘/Ctrl+Enter to save · Esc to cancel" : "Enter to save · Esc to cancel"}
        </span>
      </div>
      {edit.message && <Banner tone="error">{edit.message}</Banner>}
    </div>
  );
}
