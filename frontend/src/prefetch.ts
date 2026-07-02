// An instant.page-style, hover-intent prefetch layer shared by every in-app
// navigation that would otherwise show a loading spinner on arrival. Warming
// always routes through the shared `api` client (never a raw `fetch`), targets
// only safe, idempotent GETs, and is scoped to this authenticated dashboard —
// so the opportunistic cost of a hovered-but-not-clicked link is a single cheap
// Control Plane read.
//
// API JSON is not HTTP-cacheable, so a bare network prefetch would be wasted.
// Instead the in-flight promise is stashed in a tiny module-level cache keyed
// by an opaque string that the destination view consumes on mount, turning the
// usual loading spinner into an already-resolved response.
//
// The cache is plain in-memory JS state (a module-level Map) — it uses no
// storage API, so a full browser refresh tears down the runtime, re-runs this
// module, and the cache comes back empty. Nothing here survives a reload.

import { useCallback, useEffect, useMemo, useRef } from "react";

/** Tunable knobs for the prefetch layer. Centralised here so future tuning is a
 *  one-line change; per-call overrides are still accepted via `useHoverPrefetch`. */
export interface PrefetchTuning {
    /** Hover dwell before warming, so merely sweeping the cursor across a link
     *  does not fire a request — only lingering (an intent-to-click signal)
     *  does. */
    hoverIntentMs: number;
    /** How long a warmed entry stays usable. This cache has no invalidation
     *  channel, so its TTL is the sole freshness bound and — as the entry's
     *  data is fetched at prefetch time — the exact upper bound on how stale a
     *  warmed response can be. It must outlast the human decision gap between
     *  hovering a link and committing to the click (read the row → decide →
     *  click → route change → mount), whose long tail runs a few seconds; a
     *  miss simply falls back to a fresh fetch, so erring short only costs a
     *  spinner, never correctness. */
    ttlMs: number;
}

/** instant.page's researched default: it warms after ~65 ms of hover intent.
 *  The TTL is Serval-specific — instant.page relies on HTTP caching for
 *  freshness, but this JS cache has no invalidation hook. 3 s covers the bulk
 *  of the hover→click decision-time distribution (whose median is ~300 ms but
 *  whose deliberate tail runs to a few seconds) while keeping the worst-case
 *  staleness against a concurrent admin edit trivially small. */
export const PREFETCH_DEFAULTS: PrefetchTuning = {
    hoverIntentMs: 65,
    ttlMs: 3_000,
};

/** Stable cache keys, namespaced so distinct resources never collide. */
export const prefetchKey = {
    snippetsList: () => "snippets:list",
    snippetDetail: (id: string) => `snippet:${id}`,
} as const;

interface Entry {
    promise: Promise<unknown>;
    createdAt: number;
}

const cache = new Map<string, Entry>();

function isFresh(entry: Entry | undefined, ttlMs: number): entry is Entry {
    return entry !== undefined && Date.now() - entry.createdAt < ttlMs;
}

/** Warm the cache for `key` by invoking `loader`. Idempotent and best-effort:
 *  at most one in-flight request per key within the TTL window, and a failed
 *  warm simply drops the entry so the real navigation fetches normally. */
export function prefetch(
    key: string,
    loader: () => Promise<unknown>,
    ttlMs: number = PREFETCH_DEFAULTS.ttlMs,
): void {
    if (isFresh(cache.get(key), ttlMs)) {
        return;
    }
    const promise = loader();
    // Swallow rejections here so an unclicked prefetch never surfaces an
    // unhandled rejection; drop the entry so navigation retries against the
    // network rather than replaying the failure.
    promise.catch(() => cache.delete(key));
    cache.set(key, { promise, createdAt: Date.now() });
}

/** Resolve `key`, preferring a fresh prefetched response and falling back to
 *  `loader` — including when a prefetched request rejected. Prefetched entries
 *  are single-use, so a later visit always re-fetches. */
export async function loadPrefetched<T>(
    key: string,
    loader: () => Promise<T>,
    ttlMs: number = PREFETCH_DEFAULTS.ttlMs,
): Promise<T> {
    const entry = cache.get(key);
    cache.delete(key);
    if (!isFresh(entry, ttlMs)) {
        return loader();
    }
    try {
        return (await entry.promise) as T;
    } catch {
        return loader();
    }
}

/** DOM handlers that warm `key` on hover intent, keyboard focus, or
 *  pointer-down. Spread onto any interactive element so every path toward a
 *  click starts the load a beat early. Per-call `tuning` overrides the shared
 *  defaults when a specific link warrants a different dwell or TTL. */
export function useHoverPrefetch(
    key: string,
    loader: () => Promise<unknown>,
    tuning: Partial<PrefetchTuning> = {},
) {
    const { hoverIntentMs, ttlMs } = { ...PREFETCH_DEFAULTS, ...tuning };
    const timer = useRef<number | null>(null);

    // Keep the latest loader without re-creating handlers each render.
    const loaderRef = useRef(loader);
    useEffect(() => {
        loaderRef.current = loader;
    }, [loader]);

    const cancel = useCallback(() => {
        if (timer.current !== null) {
            window.clearTimeout(timer.current);
            timer.current = null;
        }
    }, []);

    const warmAfterDwell = useCallback(() => {
        cancel();
        timer.current = window.setTimeout(() => {
            timer.current = null;
            prefetch(key, () => loaderRef.current(), ttlMs);
        }, hoverIntentMs);
    }, [cancel, key, hoverIntentMs, ttlMs]);

    const warmNow = useCallback(() => {
        cancel();
        prefetch(key, () => loaderRef.current(), ttlMs);
    }, [cancel, key, ttlMs]);

    // Clear any pending dwell timer if the element unmounts mid-hover.
    useEffect(() => cancel, [cancel]);

    return useMemo(
        () => ({
            onMouseEnter: warmAfterDwell,
            onMouseLeave: cancel,
            onFocus: warmNow,
            onPointerDown: warmNow,
        }),
        [warmAfterDwell, cancel, warmNow],
    );
}
