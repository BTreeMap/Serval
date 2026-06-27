import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  api,
  ApiError,
  deliveryUrl,
  type CreateRequest,
  type SnippetResponse,
  type SnippetSummary,
} from "./api";
import { Badge, Button, Card, CopyButton, ErrorBanner } from "./ui";

/** The landing page: a creation form above the caller's existing snippets. */
export function Dashboard() {
  const [snippets, setSnippets] = useState<SnippetSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnippets(await api.listSnippets());
      setError(null);
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // `refresh` only updates state after an awaited request, so the renders
    // are not the synchronous cascade this rule guards against.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void refresh();
  }, [refresh]);

  return (
    <div className="space-y-8">
      <CreateForm onCreated={() => void refresh()} />

      <section className="space-y-4">
        <h2 className="text-lg font-semibold">Your snippets</h2>
        {error && <ErrorBanner message={error} />}
        {loading ? (
          <p className="text-sm text-slate-400">Loading…</p>
        ) : snippets.length === 0 ? (
          <p className="text-sm text-slate-400">
            No snippets yet. Create one above to get started.
          </p>
        ) : (
          <ul className="space-y-3">
            {snippets.map((s) => (
              <SnippetRow key={s.id} snippet={s} />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

/** A single row in the snippet list. */
function SnippetRow({ snippet }: { snippet: SnippetSummary }) {
  return (
    <li>
      <Card className="flex items-center justify-between gap-4 p-4">
        <div className="min-w-0">
          <Link
            to={`/s/${snippet.id}`}
            className="block truncate font-mono text-sm text-sky-300 hover:underline"
          >
            {snippet.id}
          </Link>
          <div className="mt-1 flex items-center gap-2 text-xs text-slate-400">
            <Badge tone={snippet.immutable ? "amber" : "sky"}>
              {snippet.immutable ? "immutable" : "mutable"}
            </Badge>
            <span>{snippet.content_type}</span>
            <span>·</span>
            <span>updated {formatDate(snippet.updated_at)}</span>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <CopyButton value={deliveryUrl(snippet.id)} label="Copy link" />
          <Link to={`/s/${snippet.id}`}>
            <Button variant="ghost">Details</Button>
          </Link>
        </div>
      </Card>
    </li>
  );
}

/** The snippet creation form. */
function CreateForm({ onCreated }: { onCreated: () => void }) {
  const [content, setContent] = useState("");
  const [contentType, setContentType] = useState("");
  const [immutable, setImmutable] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<SnippetResponse | null>(null);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!content) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const payload: CreateRequest = { content, immutable };
      if (contentType.trim()) {
        payload.content_type = contentType.trim();
      }
      const result = await api.createSnippet(payload);
      setCreated(result);
      setContent("");
      setContentType("");
      onCreated();
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card>
      <h2 className="text-lg font-semibold">Create a snippet</h2>
      <p className="mt-1 text-sm text-slate-400">
        Templates support <code className="text-sky-300">{"{{variable}}"}</code>{" "}
        placeholders, substituted from the delivery URL query string.
      </p>
      <form onSubmit={(e) => void submit(e)} className="mt-4 space-y-4">
        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          placeholder="Hello {{name}} on port {{port}}"
          rows={6}
          className="w-full resize-y rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 font-mono text-sm text-slate-100 focus:border-sky-500 focus:outline-none"
        />
        <div className="flex flex-wrap items-center gap-4">
          <input
            value={contentType}
            onChange={(e) => setContentType(e.target.value)}
            placeholder="content type (optional)"
            className="flex-1 rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 focus:border-sky-500 focus:outline-none"
          />
          <label className="flex items-center gap-2 text-sm text-slate-300">
            <input
              type="checkbox"
              checked={immutable}
              onChange={(e) => setImmutable(e.target.checked)}
              className="h-4 w-4 rounded border-slate-600 bg-slate-950"
            />
            Immutable permalink
          </label>
          <Button type="submit" disabled={busy}>
            {busy ? "Creating…" : "Create"}
          </Button>
        </div>
        {error && <ErrorBanner message={error} />}
      </form>

      {created && (
        <div className="mt-4 rounded-lg border border-sky-500/30 bg-sky-500/10 p-4">
          <p className="text-sm text-slate-200">Created successfully.</p>
          <div className="mt-2 flex items-center gap-2">
            <code className="flex-1 truncate font-mono text-xs text-sky-300">
              {deliveryUrl(created.id)}
            </code>
            <CopyButton value={deliveryUrl(created.id)} label="Copy link" />
          </div>
        </div>
      )}
    </Card>
  );
}

/** Extract a user-facing message from an unknown error. */
function messageOf(err: unknown): string {
  if (err instanceof ApiError) {
    return err.message;
  }
  return "Something went wrong. Please try again.";
}

/** Render an ISO timestamp as a short local string. */
function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}
