import { useEffect, useRef, useState } from "react";
import { Button } from "./Button";
import { Banner } from "./feedback";
import * as Icons from "./icons";

/** A seamless, Notion-style inline field. In display mode it reads as plain
 *  text — or a faint placeholder when empty — and reveals an edit affordance on
 *  hover. Clicking turns it into a borderless control that matches the
 *  displayed typography exactly (via `displayClass`), so editing feels in-place
 *  rather than form-like. Saving is always explicit: Enter (⌘/Ctrl+Enter for
 *  multiline) or the Save button; Escape cancels. Saving an empty value yields
 *  an empty string to `onSave`, leaving the clearing semantics to the caller. */
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
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement | null>(null);

  const open = () => {
    setDraft(value ?? "");
    setError(null);
    setEditing(true);
  };

  const cancel = () => {
    setEditing(false);
    setError(null);
  };

  // On entering edit mode, focus the control and place the caret at the end.
  useEffect(() => {
    if (!editing) {
      return;
    }
    const el = inputRef.current;
    if (!el) {
      return;
    }
    el.focus();
    const end = el.value.length;
    el.setSelectionRange(end, end);
  }, [editing]);

  const save = async () => {
    const next = draft.trim();
    if (next === (value ?? "")) {
      cancel();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSave(next);
      setEditing(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Something went wrong. Please try again.");
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      cancel();
    } else if (event.key === "Enter" && (!multiline || event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      void save();
    }
  };

  if (!editing) {
    return (
      <button
        type="button"
        onClick={open}
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
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
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
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          aria-label={ariaLabel}
          className={controlClass}
        />
      )}
      <div className="flex flex-wrap items-center gap-2 px-2">
        <Button size="sm" loading={busy} onClick={() => void save()}>
          {busy ? "Saving…" : "Save"}
        </Button>
        <Button variant="ghost" size="sm" onClick={cancel} disabled={busy}>
          Cancel
        </Button>
        <span className="text-xs text-ink-faint">
          {multiline ? "⌘/Ctrl+Enter to save · Esc to cancel" : "Enter to save · Esc to cancel"}
        </span>
      </div>
      {error && <Banner tone="error">{error}</Banner>}
    </div>
  );
}
