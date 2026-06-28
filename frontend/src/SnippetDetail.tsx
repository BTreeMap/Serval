import { useCallback, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import {
  api,
  ApiError,
  deliveryUrl,
  type HistoryItem,
  type SnippetDetail as Detail,
} from "./api";
import { Badge, Button, Card, CopyButton, ErrorBanner } from "./ui";

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

  if (loading) {
    return <p className="text-sm text-ink-soft">Loading…</p>;
  }
  if (error) {
    return (
      <div className="space-y-4">
        <ErrorBanner message={error} />
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

      <Card className="space-y-3">
        <div className="flex items-center justify-between gap-4">
          <code className="truncate font-mono text-sm text-wisteria-deep">
            {detail.id}
          </code>
        </div>
        <div className="flex items-center gap-2 text-xs text-ink-soft">
          <span>{detail.content_type}</span>
          <span>·</span>
          <span>{detail.history_count} version(s)</span>
        </div>
        <div className="flex items-center gap-2">
          <code className="flex-1 truncate rounded bg-canvas px-3 py-2 font-mono text-xs text-ink-soft">
            {deliveryUrl(detail.id)}
          </code>
          <CopyButton value={deliveryUrl(detail.id)} label="Copy link" />
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
      await api.updateSnippet(id, content);
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
        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          placeholder="New template content…"
          rows={6}
          className="w-full resize-y rounded-lg border border-line bg-canvas px-3 py-2 font-mono text-sm text-ink focus:border-wisteria focus:outline-none"
        />
        {error && <ErrorBanner message={error} />}
        <Button type="submit" disabled={busy}>
          {busy ? "Publishing…" : "Publish update"}
        </Button>
      </form>
    </Card>
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
      <h2 className="text-lg font-semibold">Version history</h2>
      {error && <ErrorBanner message={error} />}
      <ol className="space-y-2">
        {history.map((entry, index) => {
          const isCurrent = index === 0;
          const isOpen = preview?.hash === entry.target_hash;
          return (
            <li
              key={`${entry.changed_at}-${entry.target_hash}`}
              className="space-y-3 rounded-lg border border-line bg-surface px-4 py-3"
            >
              <div className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <code className="block truncate font-mono text-xs text-ink-soft">
                    {entry.target_hash}
                  </code>
                  <span className="text-xs text-ink-faint">
                    by {entry.editor_id} · {formatDate(entry.changed_at)}
                  </span>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Badge tone={isCurrent ? "wisteria" : "neutral"}>
                    {isCurrent ? "current" : `v${history.length - index}`}
                  </Badge>
                  <Button variant="ghost" onClick={() => void view(entry.target_hash)}>
                    {isOpen ? "Hide" : "View"}
                  </Button>
                  {!isCurrent && (
                    <Button
                      variant="secondary"
                      disabled={busyHash === entry.target_hash}
                      onClick={() => void restore(entry.target_hash)}
                    >
                      {busyHash === entry.target_hash ? "Restoring…" : "Restore"}
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
    <Link to="/" className="text-sm text-wisteria-deep hover:underline">
      ← Back to dashboard
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
