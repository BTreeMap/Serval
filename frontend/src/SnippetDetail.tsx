import { useCallback, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import {
  api,
  ApiError,
  deliveryUrl,
  type HistoryItem,
  type SnippetDetail as Detail,
} from "./api";
import { Badge, Banner, Button, Card, Combobox, CopyButton, Icons, Input, Loading, Textarea } from "./ui";
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

      <Card className="space-y-3">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            {detail.title && (
              <p className="truncate text-base font-semibold text-ink">{detail.title}</p>
            )}
            <code className="block truncate font-mono text-sm text-wisteria-deep">
              {detail.id}
            </code>
          </div>
        </div>
        <div className="space-y-1.5">
          <TitleEditor
            id={detail.id}
            value={detail.title ?? null}
            onUpdated={() => void refresh()}
          />
          <DescriptionEditor
            id={detail.id}
            value={detail.description ?? null}
            onUpdated={() => void refresh()}
          />
          <div className="flex flex-wrap items-center gap-2 text-xs text-ink-soft">
            <ContentTypeEditor
              id={detail.id}
              value={detail.content_type}
              onUpdated={() => void refresh()}
            />
            <span aria-hidden>·</span>
            <span>{detail.history_count} version(s)</span>
          </div>
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

/** Inline editor for a snippet's optional title annotation. An empty save
 *  clears the title. Appends no version to the history ledger. */
function TitleEditor({
  id,
  value,
  onUpdated,
}: {
  id: string;
  value: string | null;
  onUpdated: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const open = () => {
    setDraft(value ?? "");
    setError(null);
    setEditing(true);
  };

  const save = async () => {
    const next = draft.trim();
    if (next === (value ?? "")) {
      setEditing(false);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.updateSnippet(id, { title: next });
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
      <div className="flex items-center gap-1 text-sm">
        {value ? (
          <span className="text-ink">{value}</span>
        ) : (
          <span className="text-ink-faint">No title</span>
        )}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={open}
          title="Edit title"
          className="h-auto p-0.5"
        >
          <Icons.Pencil className="h-3.5 w-3.5" aria-hidden />
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        placeholder="Snippet title…"
        className="max-w-sm flex-1"
        aria-label="Title"
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
    </div>
  );
}

/** Inline editor for a snippet's optional description annotation. An empty
 *  save clears the description. Appends no version to the history ledger. */
function DescriptionEditor({
  id,
  value,
  onUpdated,
}: {
  id: string;
  value: string | null;
  onUpdated: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const open = () => {
    setDraft(value ?? "");
    setError(null);
    setEditing(true);
  };

  const save = async () => {
    const next = draft.trim();
    if (next === (value ?? "")) {
      setEditing(false);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.updateSnippet(id, { description: next });
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
      <div className="flex items-center gap-1 text-sm">
        {value ? (
          <span className="text-ink-soft">{value}</span>
        ) : (
          <span className="text-ink-faint">No description</span>
        )}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={open}
          title="Edit description"
          className="h-auto p-0.5"
        >
          <Icons.Pencil className="h-3.5 w-3.5" aria-hidden />
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        placeholder="Snippet description…"
        className="max-w-lg flex-1"
        aria-label="Description"
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
