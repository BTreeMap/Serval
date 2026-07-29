import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { messageOf } from "./errors";
import { NO_PAGES, concatPages, cursorAfter, loadedCount, type Page } from "./pagination";

/** Whether another page is being fetched, and why the last attempt stopped. */
export type MoreState =
    | { readonly tag: "idle" }
    | { readonly tag: "loading" }
    | { readonly tag: "failed"; readonly message: string };

const IDLE: MoreState = Object.freeze({ tag: "idle" as const });
const LOADING_MORE: MoreState = Object.freeze({ tag: "loading" as const });

export interface CursorPager<T> {
    /** Seed items followed by every appended page, in order. */
    readonly items: readonly T[];
    /** Cursor for the next page, or `null` when the collection is exhausted. */
    readonly nextCursor: string | null;
    /** How many items are loaded — cheaper than reading `items.length` when the
     *  caller only needs the count for a "n of m loaded" label. */
    readonly loaded: number;
    readonly more: MoreState;
    readonly loadMore: () => void;
}

/** Everything the pager knows, in one cell, tagged with the seed it extends.
 *
 *  The three cells this replaces (`items`, `nextCursor`, `loadingMore`, plus an
 *  error string) could disagree: a "load more" resolving after a refresh reset
 *  the list appended a page of the *old* collection onto the *new* one —
 *  duplicating rows, and in the version ledger colliding React keys. Binding
 *  the pages to their seed makes that append unrepresentable rather than
 *  merely unlikely. */
interface PagerCell<T> {
    readonly seed: Page<T> | null;
    readonly pages: readonly Page<T>[];
    readonly more: MoreState;
}

const INITIAL: PagerCell<never> = Object.freeze({
    seed: null,
    pages: NO_PAGES,
    more: IDLE,
});

/** Append further pages to a `seed` page that some other source owns.
 *
 *  `seed` is expected to be referentially stable while it represents the same
 *  fetch and to change identity when the underlying collection is re-read (as
 *  `useRemoteQuery`'s settlement does). That identity *is* the generation
 *  marker: when it changes, appended pages and any in-flight "loading" marker
 *  fall away by derivation, with no reset effect that could race the append it
 *  is meant to cancel. */
export function useCursorPager<T>(
    seed: Page<T> | null,
    fetchPage: (cursor: string, signal: AbortSignal) => Promise<Page<T>>,
): CursorPager<T> {
    const [cell, setCell] = useState<PagerCell<T>>(INITIAL);

    const fetchRef = useRef(fetchPage);
    useEffect(() => {
        fetchRef.current = fetchPage;
    });

    // Abort an append that is still in flight when the component unmounts; the
    // commit guards below already discard one whose seed was superseded, this
    // just stops paying for the bytes.
    const inFlight = useRef<AbortController | null>(null);
    useEffect(() => () => inFlight.current?.abort(), []);

    // Pages belong to a seed. A cell pointing at a superseded seed is not this
    // pager's state, so it reads as "nothing appended yet".
    const current: PagerCell<T> = cell.seed === seed ? cell : (INITIAL as PagerCell<T>);
    const { pages, more } = current;

    const items = useMemo(() => concatPages(seed, pages), [seed, pages]);
    const nextCursor = cursorAfter(seed, pages);
    const loaded = loadedCount(seed, pages);

    const loadMore = useCallback(() => {
        if (seed === null || nextCursor === null || more.tag === "loading") {
            return;
        }
        inFlight.current?.abort();
        const controller = new AbortController();
        inFlight.current = controller;

        setCell({ seed, pages, more: LOADING_MORE });
        fetchRef.current(nextCursor, controller.signal).then(
            (page) => {
                if (controller.signal.aborted) {
                    return;
                }
                // A page fetched against a seed that has since been replaced is
                // discarded on one of two paths, covering both interleavings:
                // this guard catches it once some later `loadMore` has re-seated
                // the cell onto the new seed, and the derivation above ignores
                // it otherwise, because a cell tagged with a superseded seed is
                // not this pager's state. Either way it never reaches `items`.
                setCell((prev) =>
                    prev.seed === seed && prev.more.tag === "loading"
                        ? { seed, pages: [...prev.pages, page], more: IDLE }
                        : prev,
                );
            },
            (error: unknown) => {
                // An abort is our own cancellation, not a failure to report.
                if (controller.signal.aborted) {
                    return;
                }
                setCell((prev) =>
                    prev.seed === seed && prev.more.tag === "loading"
                        ? { ...prev, more: { tag: "failed", message: messageOf(error) } }
                        : prev,
                );
            },
        );
    }, [seed, pages, nextCursor, more.tag]);

    return { items, nextCursor, loaded, more, loadMore };
}
