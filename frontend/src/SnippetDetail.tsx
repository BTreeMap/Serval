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

/** Detail view for one snippet: metadata, an editor for mutable routes, and the
 *  append-only version ledger. */
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
    return <p className="text-sm text-slate-400">Loading…</p>;
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
          <code className="truncate font-mono text-sm text-sky-300">
            {detail.id}
          </code>
          <Badge tone={detail.immutable ? "amber" : "sky"}>
            {detail.immutable ? "immutable" : "mutable"}
          </Badge>
        </div>
        <div className="flex items-center gap-2 text-xs text-slate-400">
          <span>{detail.content_type}</span>
          <span>·</span>
          <span>{detail.history_count} version(s)</span>
        </div>
        <div className="flex items-center gap-2">
          <code className="flex-1 truncate rounded bg-slate-950 px-3 py-2 font-mono text-xs text-slate-300">
            {deliveryUrl(detail.id)}
          </code>
          <CopyButton value={deliveryUrl(detail.id)} label="Copy link" />
        </div>
      </Card>

      {detail.immutable ? (
        <p className="text-sm text-slate-400">
          Immutable permalinks are content-addressed and cannot be edited.
          Create a new snippet to publish different content.
        </p>
      ) : (
        <Editor id={detail.id} onUpdated={() => void refresh()} />
      )}

      <HistoryList history={detail.history} />
    </div>
  );
}

/** An inline editor that repoints a mutable alias at new content. */
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
          className="w-full resize-y rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 font-mono text-sm text-slate-100 focus:border-sky-500 focus:outline-none"
        />
        {error && <ErrorBanner message={error} />}
        <Button type="submit" disabled={busy}>
          {busy ? "Publishing…" : "Publish update"}
        </Button>
      </form>
    </Card>
  );
}

/** The version ledger, newest first. */
function HistoryList({ history }: { history: HistoryItem[] }) {
  return (
    <section className="space-y-3">
      <h2 className="text-lg font-semibold">Version history</h2>
      <ol className="space-y-2">
        {history.map((entry, index) => (
          <li
            key={`${entry.changed_at}-${entry.target_hash}`}
            className="flex items-center justify-between gap-4 rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-3"
          >
            <div className="min-w-0">
              <code className="block truncate font-mono text-xs text-slate-300">
                {entry.target_hash}
              </code>
              <span className="text-xs text-slate-500">
                by {entry.editor_id} · {formatDate(entry.changed_at)}
              </span>
            </div>
            <Badge>v{history.length - index}</Badge>
          </li>
        ))}
      </ol>
    </section>
  );
}

function BackLink() {
  return (
    <Link to="/" className="text-sm text-sky-300 hover:underline">
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
