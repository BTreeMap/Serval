import { useCallback, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import {
  api,
  deliveryUrl,
  type CreateRequest,
  type SnippetListResponse,
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
import { COMMON_CONTENT_TYPES, DEFAULT_CONTENT_TYPE } from "./content-types";
import { loadPrefetched, prefetchKey, useHoverPrefetch } from "./prefetch";
import { messageOfRemote, valueOf } from "./lib/remote-data";
import { useRemoteQuery } from "./lib/useRemoteQuery";
import { useCursorPager } from "./lib/useCursorPager";
import type { Page } from "./lib/pagination";
import { messageOf } from "./lib/errors";
import { formatDate } from "./lib/format";

/** Adapt a listing response to the shared pagination shape. */
const toPage = (response: SnippetListResponse): Page<SnippetSummary> => ({
  items: response.snippets,
  nextCursor: response.next_cursor,
});

/** The landing page: a creation form above the caller's existing snippets. */
export function Dashboard() {
  const listKey = prefetchKey.snippetsList();

  // A first read may be answered by a link the user hovered; a revalidation —
  // the refetch after a create — must go to the network, or it could serve a
  // snapshot warmed before the snippet existed.
  const list = useRemoteQuery(listKey, (signal, isRevalidation) =>
    isRevalidation
      ? api.listSnippets({}, signal)
      : loadPrefetched(listKey, () => api.listSnippets({}, signal)),
  );

  // Keyed on the *response* rather than on the query state: a failed
  // revalidation produces a new state object carrying the same response
  // reference, so the seed's identity — and with it every page the user has
  // already loaded — survives the error, exactly as the old `snippets` cell did.
  const response = valueOf(list.state);
  const seed = useMemo(() => (response === null ? null : toPage(response)), [response]);

  const pager = useCursorPager(
    seed,
    useCallback(
      (cursor: string, signal: AbortSignal) =>
        api.listSnippets({ cursor }, signal).then(toPage),
      [],
    ),
  );

  const isLoading = list.state.tag === "loading";
  // A failure to load the list at all dominates a failure to load one more page.
  const error =
    messageOfRemote(list.state) ??
    (pager.more.tag === "failed" ? pager.more.message : null);

  return (
    <div className="space-y-8">
      <CreateForm onCreated={list.refresh} />

      <section className="space-y-4">
        <h2 className="text-lg font-semibold">Your snippets</h2>
        {error && <Banner tone="error">{error}</Banner>}
        {isLoading ? (
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
        ) : pager.items.length === 0 ? (
          <EmptyState
            icon={Icons.FileText}
            title="No snippets yet"
            description="Create your first snippet above to get a shareable delivery link."
          />
        ) : (
          <>
            <ul className="space-y-3">
              {pager.items.map((s) => (
                <SnippetRow key={s.id} snippet={s} />
              ))}
            </ul>
            {pager.nextCursor && (
              <div className="flex justify-center">
                <Button
                  variant="secondary"
                  size="sm"
                  loading={pager.more.tag === "loading"}
                  onClick={pager.loadMore}
                >
                  {pager.more.tag === "loading" ? "Loading…" : "Load more"}
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
  // Bind to the navigation affordances themselves (the title/id link and the
  // Details button) rather than the whole card: hovering an actual link is a
  // genuine intent-to-navigate signal, whereas the card also holds the
  // non-navigating Copy button and readable metadata a user may just be
  // scanning. Both links target the same detail route and share one warm.
  const prefetch = useHoverPrefetch(prefetchKey.snippetDetail(snippet.id), () =>
    api.getSnippet(snippet.id),
  );
  const url = deliveryUrl(snippet.id);
  return (
    <li>
      <Card className="flex flex-col gap-3 p-4 transition-colors hover:border-wisteria/40 sm:flex-row sm:items-center sm:justify-between sm:gap-4 md:p-5 lg:gap-6 lg:p-6">
        <div className="min-w-0">
          {snippet.title ? (
            <>
              <Link
                {...prefetch}
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
              {...prefetch}
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
          <CopyButton value={url} label="Copy link" size="sm" />
          <Link {...prefetch} to={`/s/${snippet.id}`}>
            <Button variant="secondary" size="sm">
              Details
            </Button>
          </Link>
        </div>
      </Card>
    </li>
  );
}

/** The editable fields of the creation form, as one value.
 *
 *  Grouped rather than kept as four sibling cells so that clearing the form
 *  after a successful create is a single assignment to {@link EMPTY_DRAFT}. The
 *  old four-setter reset is the shape where one forgotten field silently
 *  survives into the next snippet. */
interface Draft {
  readonly content: string;
  readonly contentType: string;
  readonly title: string;
  readonly description: string;
}

const EMPTY_DRAFT: Draft = Object.freeze({
  content: "",
  contentType: DEFAULT_CONTENT_TYPE,
  title: "",
  description: "",
});

/** The state of the in-flight submission. `created` is deliberately *not* part
 *  of this union: "the last snippet I successfully made" is a fact that
 *  outlives the current attempt, and its link stays useful while a later
 *  attempt is failing. */
type Submission =
  | { readonly tag: "idle" }
  | { readonly tag: "submitting" }
  | { readonly tag: "failed"; readonly message: string };

const IDLE: Submission = Object.freeze({ tag: "idle" as const });

/** Build the create payload, including only the fields the user actually
 *  filled in. Immutable construction — an absent field is absent from the
 *  object rather than present and empty. */
function createPayload(draft: Draft): CreateRequest {
  const contentType = draft.contentType.trim();
  const title = draft.title.trim();
  const description = draft.description.trim();
  return {
    content: draft.content,
    ...(contentType ? { content_type: contentType } : {}),
    ...(title ? { title } : {}),
    ...(description ? { description } : {}),
  };
}

/** The snippet creation form. */
function CreateForm({ onCreated }: { onCreated: () => void }) {
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [submission, setSubmission] = useState<Submission>(IDLE);
  const [created, setCreated] = useState<SnippetResponse | null>(null);

  const busy = submission.tag === "submitting";
  // Single source of truth for submittability: the handler guard and the
  // button's disabled state derive from the same predicate, so the UI can
  // never offer an action the handler would reject.
  const canSubmit = draft.content.length > 0;

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!canSubmit || busy) {
      return;
    }
    setSubmission({ tag: "submitting" });
    try {
      const result = await api.createSnippet(createPayload(draft));
      setCreated(result);
      setDraft(EMPTY_DRAFT);
      setSubmission(IDLE);
      onCreated();
    } catch (err) {
      setSubmission({ tag: "failed", message: messageOf(err) });
    }
  };

  const createdUrl = created === null ? null : deliveryUrl(created.id);

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
            value={draft.title}
            onChange={(e) => setDraft((d) => ({ ...d, title: e.target.value }))}
            placeholder="Title (optional)"
            aria-label="Snippet title"
            className="sm:flex-1"
          />
          <Input
            value={draft.description}
            onChange={(e) => setDraft((d) => ({ ...d, description: e.target.value }))}
            placeholder="Description (optional)"
            aria-label="Snippet description"
            className="sm:flex-1"
          />
        </div>
        <Textarea
          value={draft.content}
          onChange={(e) => setDraft((d) => ({ ...d, content: e.target.value }))}
          placeholder="Hello {{name}} on port {{port}}"
          rows={6}
          aria-label="Snippet content"
        />
        <div className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center sm:gap-4">
          <Combobox
            value={draft.contentType}
            onChange={(contentType) => setDraft((d) => ({ ...d, contentType }))}
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
        {submission.tag === "failed" && <Banner tone="error">{submission.message}</Banner>}
      </form>

      {created && createdUrl && (
        <div className="mt-4">
          <Banner tone="success">
            <p className="font-medium">Created successfully.</p>
            <div className="mt-2 flex items-center gap-2">
              <code className="min-w-0 flex-1 truncate font-mono text-xs text-ink">
                {createdUrl}
              </code>
              <CopyButton value={createdUrl} label="Copy link" size="sm" />
            </div>
          </Banner>
        </div>
      )}
    </Card>
  );
}
