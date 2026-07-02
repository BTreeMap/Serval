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
  Input,
  Skeleton,
  Textarea,
} from "./ui";
import { COMMON_CONTENT_TYPES } from "./content-types";
import { loadPrefetched, prefetchKey, useHoverPrefetch } from "./prefetch";

/** The landing page: a creation form above the caller's existing snippets. */
export function Dashboard() {
  const [snippets, setSnippets] = useState<SnippetSummary[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset to the first page: used on mount and after creating a snippet, so
  // a newly touched route's updated ordering is always visible immediately.
  const refresh = useCallback(async () => {
    try {
      const page = await loadPrefetched(prefetchKey.snippetsList(), () =>
        api.listSnippets(),
      );
      setSnippets(page.snippets);
      setNextCursor(page.next_cursor);
      setError(null);
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadMore = useCallback(async () => {
    if (!nextCursor) {
      return;
    }
    setLoadingMore(true);
    try {
      const page = await api.listSnippets({ cursor: nextCursor });
      setSnippets((prev) => [...prev, ...page.snippets]);
      setNextCursor(page.next_cursor);
      setError(null);
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setLoadingMore(false);
    }
  }, [nextCursor]);

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
                <Card className="flex items-center justify-between gap-4 p-4 md:p-5 lg:p-6">
                  <div className="min-w-0 flex-1 space-y-2">
                    <Skeleton className="h-4 w-48 sm:w-56 md:w-64" />
                    <Skeleton className="h-3 w-24 sm:w-32 md:w-40" />
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
          <>
            <ul className="space-y-3">
              {snippets.map((s) => (
                <SnippetRow key={s.id} snippet={s} />
              ))}
            </ul>
            {nextCursor && (
              <div className="flex justify-center">
                <Button
                  variant="secondary"
                  size="sm"
                  loading={loadingMore}
                  onClick={() => void loadMore()}
                >
                  {loadingMore ? "Loading…" : "Load more"}
                </Button>
              </div>
            )}
          </>
        )}
      </section>
    </div>
  );
}

/** A single row in the snippet list. */
function SnippetRow({ snippet }: { snippet: SnippetSummary }) {
  // Warm the detail view on hover intent so clicking through feels instant.
  const prefetch = useHoverPrefetch(prefetchKey.snippetDetail(snippet.id), () =>
    api.getSnippet(snippet.id),
  );
  return (
    <li>
      <Card
        {...prefetch}
        className="flex flex-col gap-3 p-4 transition-colors hover:border-wisteria/40 sm:flex-row sm:items-center sm:justify-between sm:gap-4 md:p-5 lg:gap-6 lg:p-6"
      >
        <div className="min-w-0">
          {snippet.title ? (
            <>
              <Link
                to={`/s/${snippet.id}`}
                className="block truncate text-sm font-medium text-wisteria-deep hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50"
              >
                {snippet.title}
              </Link>
              <code className="block truncate font-mono text-xs text-ink-faint">
                {snippet.id}
              </code>
            </>
          ) : (
            <Link
              to={`/s/${snippet.id}`}
              className="block truncate rounded font-mono text-sm text-wisteria-deep hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50"
            >
              {snippet.id}
            </Link>
          )}
          <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-ink-soft">
            {snippet.description && (
              <>
                <span className="max-w-xs truncate">{snippet.description}</span>
                <span aria-hidden>·</span>
              </>
            )}
            <span>{snippet.content_type}</span>
            <span aria-hidden>·</span>
            <span>updated {formatDate(snippet.updated_at)}</span>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <CopyButton value={deliveryUrl(snippet.id)} label="Copy link" size="sm" />
          <Link to={`/s/${snippet.id}`}>
            <Button variant="secondary" size="sm">
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
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<SnippetResponse | null>(null);

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
      const payload: CreateRequest = { content };
      if (contentType.trim()) {
        payload.content_type = contentType.trim();
      }
      if (title.trim()) {
        payload.title = title.trim();
      }
      if (description.trim()) {
        payload.description = description.trim();
      }
      const result = await api.createSnippet(payload);
      setCreated(result);
      setContent("");
      setContentType("text/plain; charset=utf-8");
      setTitle("");
      setDescription("");
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
        <div className="flex flex-col gap-3 sm:flex-row sm:gap-4">
          <Input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="Title (optional)"
            aria-label="Snippet title"
            className="sm:flex-1"
          />
          <Input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Description (optional)"
            aria-label="Snippet description"
            className="sm:flex-1"
          />
        </div>
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
          <Button
            type="submit"
            loading={busy}
            disabled={!canSubmit}
            className="w-full sm:w-auto"
          >
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
