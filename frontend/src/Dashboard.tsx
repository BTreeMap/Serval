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
import {
  Banner,
  Button,
  Card,
  Combobox,
  CopyButton,
  EmptyState,
  Icons,
  Skeleton,
  Textarea,
} from "./ui";

/** Canonical MIME types offered as create-form suggestions. Serval stores text,
 *  so the list favors the formats people actually paste as snippets: prose,
 *  markup, config and data. Each entry is the canonical IANA form (charset on
 *  the `text/*` types). The value at delivery time still prefers a
 *  filename-extension guess; this only sets the fallback stored on the route,
 *  and free text is always allowed for anything not listed here. */
const COMMON_CONTENT_TYPES = [
  "text/plain; charset=utf-8",
  "text/html; charset=utf-8",
  "text/markdown; charset=utf-8",
  "text/css; charset=utf-8",
  "text/javascript; charset=utf-8",
  "text/csv; charset=utf-8",
  "text/tab-separated-values; charset=utf-8",
  "text/xml; charset=utf-8",
  "application/json",
  "application/ld+json",
  "application/yaml",
  "application/toml",
  "application/xml",
  "application/rss+xml",
  "application/atom+xml",
  "image/svg+xml",
] as const;

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
        {error && <Banner tone="error">{error}</Banner>}
        {loading ? (
          <ul className="space-y-3">
            {[0, 1, 2].map((i) => (
              <li key={i}>
                <Card className="flex items-center justify-between gap-4 p-4">
                  <div className="min-w-0 flex-1 space-y-2">
                    <Skeleton className="h-4 w-2/3" />
                    <Skeleton className="h-3 w-1/3" />
                  </div>
                  <Skeleton className="h-8 w-24" />
                </Card>
              </li>
            ))}
          </ul>
        ) : snippets.length === 0 ? (
          <EmptyState
            icon={Icons.FileText}
            title="No snippets yet"
            description="Create your first snippet above to get a shareable delivery link."
          />
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
      <Card className="flex flex-col gap-3 p-4 transition-colors hover:border-wisteria/40 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
        <div className="min-w-0">
          <Link
            to={`/s/${snippet.id}`}
            className="block truncate rounded font-mono text-sm text-wisteria-deep hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50"
          >
            {snippet.id}
          </Link>
          <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-ink-soft">
            <span>{snippet.content_type}</span>
            <span aria-hidden>·</span>
            <span>updated {formatDate(snippet.updated_at)}</span>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <CopyButton value={deliveryUrl(snippet.id)} label="Copy link" size="sm" />
          <Link to={`/s/${snippet.id}`}>
            <Button variant="ghost" size="sm">
              Details
            </Button>
          </Link>
        </div>
      </Card>
    </li>
  );
}

/** The snippet creation form. */
function CreateForm({ onCreated }: { onCreated: () => void }) {
  const [content, setContent] = useState("");
  const [contentType, setContentType] = useState("text/plain; charset=utf-8");
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
      const payload: CreateRequest = { content };
      if (contentType.trim()) {
        payload.content_type = contentType.trim();
      }
      const result = await api.createSnippet(payload);
      setCreated(result);
      setContent("");
      setContentType("text/plain; charset=utf-8");
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
      <p className="mt-1 text-sm text-ink-soft">
        Templates support <code className="text-wisteria-deep">{"{{variable}}"}</code>{" "}
        placeholders, substituted from the delivery URL query string.
      </p>
      <form onSubmit={(e) => void submit(e)} className="mt-4 space-y-4">
        <Textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          placeholder="Hello {{name}} on port {{port}}"
          rows={6}
          aria-label="Snippet content"
        />
        <div className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center sm:gap-4">
          <Combobox
            value={contentType}
            onChange={setContentType}
            options={COMMON_CONTENT_TYPES}
            placeholder="content type"
            className="sm:flex-1"
          />
          <Button type="submit" loading={busy} className="w-full sm:w-auto">
            {busy ? "Creating…" : "Create"}
          </Button>
        </div>
        {error && <Banner tone="error">{error}</Banner>}
      </form>

      {created && (
        <div className="mt-4">
          <Banner tone="success">
            <p className="font-medium">Created successfully.</p>
            <div className="mt-2 flex items-center gap-2">
              <code className="min-w-0 flex-1 truncate font-mono text-xs text-ink">
                {deliveryUrl(created.id)}
              </code>
              <CopyButton value={deliveryUrl(created.id)} label="Copy link" size="sm" />
            </div>
          </Banner>
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
