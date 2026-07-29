import { useCallback, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { api, deliveryUrl, type SnippetDetail as Detail, type UpdateRequest } from "./api";
import {
  Banner,
  Button,
  Card,
  Combobox,
  CopyButton,
  Icons,
  InlineField,
  Loading,
  Textarea,
} from "./ui";
import { COMMON_CONTENT_TYPES } from "./content-types";
import { loadPrefetched, prefetchKey, useHoverPrefetch } from "./prefetch";
import { VersionHistory } from "./VersionHistory";
import { foldRemote } from "./lib/remote-data";
import { useRemoteQuery } from "./lib/useRemoteQuery";
import { useInlineEdit } from "./lib/useInlineEdit";
import { messageOf } from "./lib/errors";

/** Detail view for one snippet: metadata, an editor, and the append-only
 *  version ledger with per-version preview and restore. */
export function SnippetDetail() {
  const { id = "" } = useParams<{ id: string }>();
  const key = prefetchKey.snippetDetail(id);

  // See `Dashboard`: the refetch after publishing or restoring a version must
  // not be answered by a prefetch warmed before that write.
  const detail = useRemoteQuery(key, (signal, isRevalidation) =>
    isRevalidation
      ? api.getSnippet(id, {}, signal)
      : loadPrefetched(key, () => api.getSnippet(id, {}, signal)),
  );
  const { refresh } = detail;

  const updateAnnotation = useCallback(
    async (patch: UpdateRequest) => {
      await api.updateSnippet(id, patch);
      // Awaited: the inline field stays in its saving state until the refetched
      // value is on screen, so it never closes over the pre-edit text.
      await refresh();
    },
    [id, refresh],
  );

  // Total over the three states a remote read can be in. The old code needed a
  // fourth branch — `!detail` after loading finished with no error — that no
  // reachable state could produce.
  return foldRemote(detail.state, {
    loading: () => <Loading />,
    failure: (message) => (
      <div className="space-y-4">
        <Banner tone="error">{message}</Banner>
        <BackLink />
      </div>
    ),
    success: (value) => (
      <SnippetDetailView detail={value} onUpdated={refresh} onSave={updateAnnotation} />
    ),
  });
}

function SnippetDetailView({
  detail,
  onUpdated,
  onSave,
}: {
  detail: Detail;
  onUpdated: () => void;
  onSave: (patch: UpdateRequest) => Promise<void>;
}) {
  const url = deliveryUrl(detail.id);
  return (
    <div className="space-y-6">
      <BackLink />

      <Card className="space-y-4">
        <div className="space-y-0.5">
          <InlineField
            value={detail.title ?? null}
            onSave={(title) => onSave({ title })}
            placeholder="Untitled snippet"
            ariaLabel="title"
            displayClass="text-2xl font-semibold tracking-tight text-ink"
          />
          <InlineField
            value={detail.description ?? null}
            onSave={(description) => onSave({ description })}
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
          <ContentTypeEditor id={detail.id} value={detail.content_type} onUpdated={onUpdated} />
          <span aria-hidden>·</span>
          <span>{detail.history_count} version(s)</span>
        </div>

        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate rounded bg-canvas px-3 py-2 font-mono text-xs text-ink-soft">
            {url}
          </code>
          <CopyButton value={url} label="Copy link" size="sm" />
        </div>
      </Card>

      <Editor id={detail.id} onUpdated={onUpdated} />

      <VersionHistory detail={detail} onRestored={onUpdated} />
    </div>
  );
}

/** The state of an in-flight publish. Replaces a `busy` boolean beside an
 *  `error` string, whose fourth combination — failed *and* still publishing —
 *  no code path could produce. */
type Submission =
  | { readonly tag: "idle" }
  | { readonly tag: "submitting" }
  | { readonly tag: "failed"; readonly message: string };

const IDLE: Submission = Object.freeze({ tag: "idle" as const });

/** An inline editor that repoints a snippet at new content. */
function Editor({ id, onUpdated }: { id: string; onUpdated: () => void }) {
  const [content, setContent] = useState("");
  const [submission, setSubmission] = useState<Submission>(IDLE);

  const busy = submission.tag === "submitting";
  // Single source of truth for submittability: the handler guard and the
  // button's disabled state derive from the same predicate, so the UI can
  // never offer an action the handler would reject.
  const canSubmit = content.length > 0;

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!canSubmit || busy) {
      return;
    }
    setSubmission({ tag: "submitting" });
    try {
      await api.updateSnippet(id, { content });
      setContent("");
      setSubmission(IDLE);
      onUpdated();
    } catch (err) {
      setSubmission({ tag: "failed", message: messageOf(err) });
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
        {submission.tag === "failed" && <Banner tone="error">{submission.message}</Banner>}
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
  const initial = useCallback(() => value, [value]);
  // Blank is not a content type, and re-saving the current one is a no-op:
  // either way, close without a request.
  const isUnchanged = useCallback((next: string) => next === "" || next === value, [value]);
  const commit = useCallback(
    async (next: string) => {
      await api.updateSnippet(id, { content_type: next });
      onUpdated();
    },
    [id, onUpdated],
  );
  const edit = useInlineEdit({ initial, isUnchanged, commit });

  if (!edit.isOpen) {
    return (
      <Button
        type="button"
        variant="link"
        size="sm"
        onClick={edit.begin}
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
        value={edit.draft}
        onChange={edit.setDraft}
        options={COMMON_CONTENT_TYPES}
        placeholder="content type"
        className="w-64"
      />
      <Button size="sm" loading={edit.isSaving} onClick={edit.save}>
        {edit.isSaving ? "Saving…" : "Save"}
      </Button>
      <Button variant="ghost" size="sm" onClick={edit.cancel} disabled={edit.isSaving}>
        Cancel
      </Button>
      {edit.message && <Banner tone="error">{edit.message}</Banner>}
    </span>
  );
}

function BackLink() {
  // Warm the dashboard listing so returning from a detail view is instant too.
  const prefetch = useHoverPrefetch(prefetchKey.snippetsList(), () => api.listSnippets());
  return (
    <Link
      {...prefetch}
      to="/"
      className="inline-flex items-center gap-2 rounded text-sm text-wisteria-deep hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50"
    >
      <Icons.ArrowLeft className="h-4 w-4" aria-hidden />
      Back to dashboard
    </Link>
  );
}
