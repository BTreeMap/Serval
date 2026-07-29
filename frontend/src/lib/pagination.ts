// The pure algebra of cursor pagination, shared by the snippet list and the
// version ledger. Framework-free on purpose: these are the laws, `useCursorPager`
// is only the effect interpreter around them.

/** One page of a cursor-paginated collection. `nextCursor` is `null` exactly at
 *  the end of the collection — the Control Plane exposes no separate `has_more`
 *  flag, so there is no way to represent "more exists but no cursor to get it". */
export interface Page<T> {
    readonly items: readonly T[];
    readonly nextCursor: string | null;
}

/** Hoisted empty tail, so "no pages appended yet" is one stable reference and
 *  does not invalidate a `useMemo` keyed on it. */
export const NO_PAGES: readonly Page<never>[] = Object.freeze([]);

/** Hoisted empty result for the seedless case, for the same reason. */
const NO_ITEMS: readonly never[] = Object.freeze([]);

/** Flatten a seed page followed by appended pages into one list.
 *
 *  This is a fold whose identity is the empty list and whose operation is list
 *  concatenation — associative, and deliberately *not* commutative: both
 *  collections are ordered ledgers (newest-changed first; newest version first)
 *  where reordering would be a visible defect.
 *
 *  Implemented as a local loop rather than `flatMap`/`reduce`. Each `reduce`
 *  step over an array accumulator allocates a fresh array, making the fold
 *  quadratic in the number of loaded pages; the loop is linear in total items.
 *  The mutation never escapes this function, so it is observationally pure. */
export function concatPages<T>(seed: Page<T> | null, pages: readonly Page<T>[]): readonly T[] {
    if (seed === null) {
        return NO_ITEMS as readonly T[];
    }
    if (pages.length === 0) {
        return seed.items;
    }
    const out: T[] = [...seed.items];
    for (const page of pages) {
        for (const item of page.items) {
            out.push(item);
        }
    }
    return out;
}

/** The cursor that continues the collection, or `null` at the end.
 *
 *  It always comes from the *last* page loaded — the seed when nothing has been
 *  appended yet. Reading it from anywhere else (say, the seed after appending)
 *  would re-request a page already held. */
export function cursorAfter<T>(seed: Page<T> | null, pages: readonly Page<T>[]): string | null {
    if (pages.length > 0) {
        return pages[pages.length - 1].nextCursor;
    }
    return seed?.nextCursor ?? null;
}

/** Total number of items currently loaded across the seed and appended pages. */
export function loadedCount<T>(seed: Page<T> | null, pages: readonly Page<T>[]): number {
    let total = seed?.items.length ?? 0;
    for (const page of pages) {
        total += page.items.length;
    }
    return total;
}
