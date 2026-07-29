import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  deliveryUrl,
  type HistoryItem,
  type HistoryPageResponse,
  type SnippetDetail as Detail,
} from "./api";
import { Badge, Banner, Button, CopyButton, Icons } from "./ui";
import { messageOf } from "./lib/errors";
import { formatDate } from "./lib/format";
import { useCursorPager } from "./lib/useCursorPager";
import type { Page } from "./lib/pagination";

const toPage = (response: HistoryPageResponse): Page<HistoryItem> => ({
  items: response.history,
  nextCursor: response.next_cursor,
});

/** The version ledger, newest first. Only the newest page arrives with the
 *  detail; older entries are fetched a page at a time. Each entry can be
 *  previewed and restored; restoring repoints the snippet and appends a new
 *  version, which replaces the detail and so resets pagination back to the
 *  newest page — that reset is a consequence of the seed's identity changing,
 *  not a separate effect that could race the append it must cancel. */
export function VersionHistory({
  detail,
  onRestored,
}: {
  detail: Detail;
  onRestored: () => void;
}) {
  const id = detail.id;

  // Keyed on `detail`: one stable seed per detail fetch.
  const seed = useMemo<Page<HistoryItem>>(
    () => ({ items: detail.history, nextCursor: detail.history_next_cursor }),
    [detail],
  );

  const pager = useCursorPager(
    seed,
    useCallback(
      (cursor: string, signal: AbortSignal) =>
        api.listSnippetHistory(id, { cursor }, signal).then(toPage),
      [id],
    ),
  );

  const [openHash, setOpenHash] = useState<string | null>(null);
  const [busyHash, setBusyHash] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Per-hash content cache keyed by `target_hash`. Caching the *promise* (not
  // the resolved string) collapses concurrent View + Copy clicks into a single
  // in-flight request, and living in a ref means a cache fill triggers no
  // re-render — only the action that awaits it observes the bytes. Caching
  // without expiry is sound because a `target_hash` is a content address: the
  // bytes behind one can never change.
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

  // Takes the row's own `isOpen` rather than reading `openHash`, so its
  // identity depends only on `id`. Depending on `openHash` would give every row
  // a new callback on every toggle, re-rendering the whole unbounded ledger and
  // defeating the `memo` on the rows.
  const toggleView = useCallback(
    async (hash: string, isOpen: boolean) => {
      setError(null);
      if (isOpen) {
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
    [loadVersion],
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

  const loadingMore = pager.more.tag === "loading";

  return (
    <section className="space-y-3">
      <h2 className="flex items-center gap-2 text-lg font-semibold">
        <Icons.History className="h-5 w-5 text-ink-faint" aria-hidden />
        Version history
        <span className="text-sm font-normal text-ink-faint">
          ({pager.loaded} of {detail.history_count} loaded)
        </span>
      </h2>
      {error && <Banner tone="error">{error}</Banner>}
      <ol className="space-y-2">
        {pager.items.map((entry) => (
          <VersionHistoryRow
            key={entry.version_number}
            entry={entry}
            isOpen={openHash === entry.target_hash}
            busy={busyHash === entry.target_hash}
            loadVersion={loadVersion}
            onToggleView={toggleView}
            onRestore={restore}
          />
        ))}
      </ol>
      {pager.more.tag === "failed" && <Banner tone="error">{pager.more.message}</Banner>}
      {pager.nextCursor && (
        <div className="flex justify-center">
          <Button
            variant="secondary"
            size="sm"
            loading={loadingMore}
            onClick={pager.loadMore}
          >
            {loadingMore ? "Loading…" : "Load older versions"}
          </Button>
        </div>
      )}
    </section>
  );
}

/** One row of the version ledger. Pure and memoized: the ledger is append-only
 *  and unbounded, so stable rows keep a single toggle from re-rendering the list.
 *  Every callback prop is now identity-stable across a toggle, which is what
 *  makes the memo actually hold.
 *
 *  The action surface is split into two physically isolated, ARIA-labelled zones:
 *  Zone 1 (state & mutation) couples the version badge to the conditional restore
 *  control; Zone 2 (invariant read-only) keeps Copy link / Copy content / View in
 *  a fixed mutual order across every breakpoint. */
const VersionHistoryRow = memo(function VersionHistoryRow({
  entry,
  isOpen,
  busy,
  loadVersion,
  onToggleView,
  onRestore,
}: {
  entry: HistoryItem;
  isOpen: boolean;
  busy: boolean;
  loadVersion: (hash: string) => Promise<string>;
  onToggleView: (hash: string, isOpen: boolean) => void;
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
            <Badge tone={entry.is_current ? "wisteria" : "neutral"}>
              {entry.is_current ? "current" : `v${entry.version_number}`}
            </Badge>
            {!entry.is_current && (
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
          <Button
            variant="secondary"
            size="sm"
            onClick={() => onToggleView(entry.target_hash, isOpen)}
          >
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
