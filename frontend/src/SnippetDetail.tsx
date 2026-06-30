import { memo, useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import {
  api,
  ApiError,
  deliveryUrl,
  type HistoryItem,
  type SnippetDetail as Detail,
} from "./api";
import { Badge, Banner, Button, Card, Combobox, CopyButton, Icons, InlineField, Loading, Textarea } from "./ui";
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

  // Single source of truth for submittability: the handler guard and the
  // button's disabled state derive from the same predicate, so the UI can
  // never offer an action the handler would reject.
  const canSubmit = content.length > 0;

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!canSubmit) {
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
        <Button type="submit" loading={busy} disabled={!canSubmit}>
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

/** A version's lifecycle stance within the append-only ledger. The newest
 *  entry is `current`; everything before it is `historic` and restorable. */
type VersionState = "current" | "historic";

/** Total function from ledger position to lifecycle state — index 0 is the
 *  live pointer, the single source of truth for the badge and restore affordance. */
function determineVersionState(index: number): VersionState {
  return index === 0 ? "current" : "historic";
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
  const [openHash, setOpenHash] = useState<string | null>(null);
  const [busyHash, setBusyHash] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Per-hash content cache keyed by `target_hash`. Caching the *promise* (not
  // the resolved string) collapses concurrent View + Copy clicks into a single
  // in-flight request, and living in a ref means a cache fill triggers no
  // re-render — only the action that awaits it observes the bytes.
  const contentCache = useRef(new Map<string, Promise<string>>());

  const loadVersion = useCallback(
    (hash: string): Promise<string> => {
      const cache = contentCache.current;
      let pending = cache.get(hash);
      if (!pending) {
        pending = api.getVersion(id, hash).then((version) => version.content);
        // Evict on failure so a transient error can be retried.
        pending.catch(() => cache.delete(hash));
        cache.set(hash, pending);
      }
      return pending;
    },
    [id],
  );

  const toggleView = useCallback(
    async (hash: string) => {
      setError(null);
      if (openHash === hash) {
        setOpenHash(null);
        return;
      }
      try {
        await loadVersion(hash);
        setOpenHash(hash);
      } catch (err) {
        setError(messageOf(err));
      }
    },
    [openHash, loadVersion],
  );

  const restore = useCallback(
    async (hash: string) => {
      setError(null);
      setBusyHash(hash);
      try {
        await api.restoreVersion(id, hash);
        setOpenHash(null);
        onRestored();
      } catch (err) {
        setError(messageOf(err));
      } finally {
        setBusyHash(null);
      }
    },
    [id, onRestored],
  );

  return (
    <section className="space-y-3">
      <h2 className="flex items-center gap-2 text-lg font-semibold">
        <Icons.History className="h-5 w-5 text-ink-faint" aria-hidden />
        Version history
      </h2>
      {error && <Banner tone="error">{error}</Banner>}
      <ol className="space-y-2">
        {history.map((entry, index) => (
          <VersionHistoryRow
            key={`${entry.changed_at}-${entry.target_hash}`}
            entry={entry}
            state={determineVersionState(index)}
            label={index === 0 ? "current" : `v${history.length - index}`}
            isOpen={openHash === entry.target_hash}
            busy={busyHash === entry.target_hash}
            loadVersion={loadVersion}
            onToggleView={toggleView}
            onRestore={restore}
          />
        ))}
      </ol>
    </section>
  );
}

/** One row of the version ledger. Pure and memoized: the ledger is append-only
 *  and unbounded, so stable rows keep a single toggle from re-rendering the list.
 *
 *  The action surface is split into two physically isolated, ARIA-labelled zones:
 *  Zone 1 (state & mutation) couples the version badge to the conditional restore
 *  control; Zone 2 (invariant read-only) keeps Copy link / Copy content / View in
 *  a fixed mutual order across every breakpoint. */
const VersionHistoryRow = memo(function VersionHistoryRow({
  entry,
  state,
  label,
  isOpen,
  busy,
  loadVersion,
  onToggleView,
  onRestore,
}: {
  entry: HistoryItem;
  state: VersionState;
  label: string;
  isOpen: boolean;
  busy: boolean;
  loadVersion: (hash: string) => Promise<string>;
  onToggleView: (hash: string) => void;
  onRestore: (hash: string) => void;
}) {
  const [content, setContent] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) {
      return;
    }
    let active = true;
    // The promise is already cached by the parent; this only reads it.
    void loadVersion(entry.target_hash).then((text) => {
      if (active) {
        setContent(text);
      }
    });
    return () => {
      active = false;
    };
  }, [isOpen, entry.target_hash, loadVersion]);

  return (
    <li className="space-y-3 rounded-lg border border-line bg-surface px-4 py-3 transition-colors hover:border-wisteria/40 md:px-5 md:py-4 lg:px-6">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
        {/* Zone 1 — State & Mutation. The restore control sits beside the badge
            so the state-changing action is anchored to the state indicator. */}
        <div className="min-w-0 space-y-1.5" role="group" aria-label="version state">
          <div className="min-w-0">
            <code className="block truncate font-mono text-xs text-ink-soft">
              {entry.target_hash}
            </code>
            <span className="text-xs text-ink-faint">
              by {entry.editor_id} · {formatDate(entry.changed_at)}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <Badge tone={state === "current" ? "wisteria" : "neutral"}>{label}</Badge>
            {state === "historic" && (
              <Button
                variant="secondary"
                size="sm"
                loading={busy}
                onClick={() => onRestore(entry.target_hash)}
              >
                {busy ? (
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

        {/* Zone 2 — Invariant Read-Only. A dedicated flex container fixes the
            mutual order of these actions across all viewport sizes. */}
        <div
          className="flex shrink-0 flex-wrap items-center gap-2"
          role="group"
          aria-label="snippet actions"
        >
          <CopyButton value={deliveryUrl(entry.target_hash)} label="Copy link" size="sm" />
          <CopyButton
            load={() => loadVersion(entry.target_hash)}
            label="Copy content"
            size="sm"
          />
          <Button variant="secondary" size="sm" onClick={() => onToggleView(entry.target_hash)}>
            {isOpen ? (
              <Icons.EyeOff className="h-4 w-4" aria-hidden />
            ) : (
              <Icons.Eye className="h-4 w-4" aria-hidden />
            )}
            {isOpen ? "Hide" : "View"}
          </Button>
        </div>
      </div>
      {isOpen && content !== null && (
        <pre className="overflow-x-auto rounded bg-canvas px-3 py-2 font-mono text-xs text-ink">
          {content}
        </pre>
      )}
    </li>
  );
});

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
