import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import {
  api,
  ApiError,
  deliveryUrl,
  type HistoryItem,
  type SnippetDetail as Detail,
} from "./api";
import { Badge, Banner, Button, Card, Combobox, CopyButton, Icons, Loading, Textarea } from "./ui";
import { COMMON_CONTENT_TYPES } from "./content-types";

/** Detail view for one snippet: metadata, an editor, and the append-only
 *  version ledger with per-version preview and restore. */
export function SnippetDetail() {
  const { id = "" } = useParams<{ id: string }>();
  const [detail, setDetail] = useState<Detail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setDetail(await api.getSnippet(id));
      setError(null);
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    // `refresh` only updates state after an awaited request, so the renders
    // are not the synchronous cascade this rule guards against.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void refresh();
  }, [refresh]);

  const updateAnnotation = useCallback(
    async (patch: { title: string } | { description: string }) => {
      await api.updateSnippet(id, patch);
      await refresh();
    },
    [id, refresh],
  );

  if (loading) {
    return <Loading />;
  }
  if (error) {
    return (
      <div className="space-y-4">
        <Banner tone="error">{error}</Banner>
        <BackLink />
      </div>
    );
  }
  if (!detail) {
    return <BackLink />;
  }

  return (
    <div className="space-y-6">
      <BackLink />

      <Card className="space-y-4">
        <div className="space-y-0.5">
          <InlineField
            value={detail.title ?? null}
            onSave={(next) => updateAnnotation({ title: next })}
            placeholder="Untitled snippet"
            ariaLabel="title"
            displayClass="text-2xl font-semibold tracking-tight text-ink"
          />
          <InlineField
            value={detail.description ?? null}
            onSave={(next) => updateAnnotation({ description: next })}
            placeholder="Add a description…"
            ariaLabel="description"
            multiline
            rows={3}
            displayClass="text-sm leading-relaxed text-ink-soft"
          />
        </div>

        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-ink-soft">
          <code className="truncate font-mono text-wisteria-deep">{detail.id}</code>
          <span aria-hidden>·</span>
          <ContentTypeEditor
            id={detail.id}
            value={detail.content_type}
            onUpdated={() => void refresh()}
          />
          <span aria-hidden>·</span>
          <span>{detail.history_count} version(s)</span>
        </div>

        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate rounded bg-canvas px-3 py-2 font-mono text-xs text-ink-soft">
            {deliveryUrl(detail.id)}
          </code>
          <CopyButton value={deliveryUrl(detail.id)} label="Copy link" size="sm" />
        </div>
      </Card>

      <Editor id={detail.id} onUpdated={() => void refresh()} />

      <HistoryList
        id={detail.id}
        history={detail.history}
        onRestored={() => void refresh()}
      />
    </div>
  );
}

/** An inline editor that repoints a snippet at new content. */
function Editor({ id, onUpdated }: { id: string; onUpdated: () => void }) {
  const [content, setContent] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!content) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.updateSnippet(id, { content });
      setContent("");
      onUpdated();
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card>
      <h2 className="text-lg font-semibold">Publish a new version</h2>
      <form onSubmit={(e) => void submit(e)} className="mt-4 space-y-4">
        <Textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          placeholder="New template content…"
          rows={6}
          aria-label="New version content"
        />
        {error && <Banner tone="error">{error}</Banner>}
        <Button type="submit" loading={busy}>
          {busy ? "Publishing…" : "Publish update"}
        </Button>
      </form>
    </Card>
  );
}

/** Inline editor for a snippet's stored `content_type`. Changing it is pure
 *  route metadata — it appends no version to the history ledger. */
function ContentTypeEditor({
  id,
  value,
  onUpdated,
}: {
  id: string;
  value: string;
  onUpdated: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const open = () => {
    setDraft(value);
    setError(null);
    setEditing(true);
  };

  const save = async () => {
    const next = draft.trim();
    if (!next || next === value) {
      setEditing(false);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.updateSnippet(id, { content_type: next });
      setEditing(false);
      onUpdated();
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  };

  if (!editing) {
    return (
      <Button
        type="button"
        variant="link"
        size="sm"
        onClick={open}
        className="font-mono"
        title="Edit content type"
      >
        {value}
      </Button>
    );
  }

  return (
    <span className="flex flex-wrap items-center gap-2">
      <Combobox
        value={draft}
        onChange={setDraft}
        options={COMMON_CONTENT_TYPES}
        placeholder="content type"
        className="w-64"
      />
      <Button size="sm" loading={busy} onClick={() => void save()}>
        {busy ? "Saving…" : "Save"}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setEditing(false)}
        disabled={busy}
      >
        Cancel
      </Button>
      {error && <Banner tone="error">{error}</Banner>}
    </span>
  );
}

/** A seamless, Notion-style inline field. In display mode it reads as plain
 *  text — or a faint placeholder when empty — and reveals an edit affordance on
 *  hover. Clicking turns it into a borderless input that matches the displayed
 *  typography exactly, so editing feels in-place rather than form-like. Saving
 *  is always explicit: Enter (⌘/Ctrl+Enter for multiline) or the Save button;
 *  Escape cancels. Saving an empty value clears the annotation. Changing an
 *  annotation appends no version to the history ledger. */
function InlineField({
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
      setError(messageOf(err));
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

/** The version ledger, newest first. Each entry can be previewed and restored;
 *  restoring repoints the snippet and appends a new version. */
function HistoryList({
  id,
  history,
  onRestored,
}: {
  id: string;
  history: HistoryItem[];
  onRestored: () => void;
}) {
  const [preview, setPreview] = useState<{ hash: string; content: string } | null>(
    null,
  );
  const [busyHash, setBusyHash] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const view = async (hash: string) => {
    setError(null);
    if (preview?.hash === hash) {
      setPreview(null);
      return;
    }
    try {
      const version = await api.getVersion(id, hash);
      setPreview({ hash, content: version.content });
    } catch (err) {
      setError(messageOf(err));
    }
  };

  const restore = async (hash: string) => {
    setError(null);
    setBusyHash(hash);
    try {
      await api.restoreVersion(id, hash);
      setPreview(null);
      onRestored();
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setBusyHash(null);
    }
  };

  return (
    <section className="space-y-3">
      <h2 className="flex items-center gap-2 text-lg font-semibold">
        <Icons.History className="h-5 w-5 text-ink-faint" aria-hidden />
        Version history
      </h2>
      {error && <Banner tone="error">{error}</Banner>}
      <ol className="space-y-2">
        {history.map((entry, index) => {
          const isCurrent = index === 0;
          const isOpen = preview?.hash === entry.target_hash;
          return (
            <li
              key={`${entry.changed_at}-${entry.target_hash}`}
              className="space-y-3 rounded-lg border border-line bg-surface px-4 py-3 transition-colors hover:border-wisteria/40 md:px-5 md:py-4 lg:px-6"
            >
              <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 lg:gap-6">
                <div className="min-w-0">
                  <code className="block truncate font-mono text-xs text-ink-soft">
                    {entry.target_hash}
                  </code>
                  <span className="text-xs text-ink-faint">
                    by {entry.editor_id} · {formatDate(entry.changed_at)}
                  </span>
                </div>
                <div className="flex shrink-0 flex-wrap items-center gap-2">
                  <Badge tone={isCurrent ? "wisteria" : "neutral"}>
                    {isCurrent ? "current" : `v${history.length - index}`}
                  </Badge>
                  <CopyButton
                    value={deliveryUrl(entry.target_hash)}
                    label="Copy link"
                    size="sm"
                  />
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => void view(entry.target_hash)}
                  >
                    {isOpen ? (
                      <Icons.EyeOff className="h-4 w-4" aria-hidden />
                    ) : (
                      <Icons.Eye className="h-4 w-4" aria-hidden />
                    )}
                    {isOpen ? "Hide" : "View"}
                  </Button>
                  {!isCurrent && (
                    <Button
                      variant="secondary"
                      size="sm"
                      loading={busyHash === entry.target_hash}
                      onClick={() => void restore(entry.target_hash)}
                    >
                      {busyHash === entry.target_hash ? (
                        "Restoring…"
                      ) : (
                        <>
                          <Icons.RotateCcw className="h-4 w-4" aria-hidden />
                          Restore
                        </>
                      )}
                    </Button>
                  )}
                </div>
              </div>
              {isOpen && (
                <pre className="overflow-x-auto rounded bg-canvas px-3 py-2 font-mono text-xs text-ink">
                  {preview.content}
                </pre>
              )}
            </li>
          );
        })}
      </ol>
    </section>
  );
}

function BackLink() {
  return (
    <Link
      to="/"
      className="inline-flex items-center gap-2 rounded text-sm text-wisteria-deep hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50"
    >
      <Icons.ArrowLeft className="h-4 w-4" aria-hidden />
      Back to dashboard
    </Link>
  );
}

function messageOf(err: unknown): string {
  if (err instanceof ApiError) {
    return err.message;
  }
  return "Something went wrong. Please try again.";
}

function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}
