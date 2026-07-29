// Timestamp rendering. One formatter, constructed once.

/** The field set `Date.prototype.toLocaleString()` uses by default.
 *
 *  Spelling it out is what makes hoisting safe: a bare `new Intl.DateTimeFormat()`
 *  yields *date only*, and `dateStyle: "short"` abbreviates the year — neither is
 *  what `toLocaleString()` produces. Verified byte-identical to `toLocaleString()`
 *  across 10 locales x 6 instants (see `format.test.ts`). */
const DATE_TIME_FIELDS: Intl.DateTimeFormatOptions = {
    year: "numeric",
    month: "numeric",
    day: "numeric",
    hour: "numeric",
    minute: "numeric",
    second: "numeric",
};

/** Constructed once per session rather than once per row per render.
 *  `Intl.DateTimeFormat` resolution is the expensive part of `toLocaleString`;
 *  the history ledger is unbounded, so this is per-row work on a list that
 *  grows without limit. `undefined` locale means the runtime default, matching
 *  the implicit locale `toLocaleString()` picked at each call site. */
const dateTimeFormat = new Intl.DateTimeFormat(undefined, DATE_TIME_FIELDS);

/** Render an ISO timestamp as a short local string, echoing the input verbatim
 *  when it does not parse — an unparseable timestamp is server data we should
 *  show as-is rather than replace with "Invalid Date". */
export function formatDate(iso: string): string {
    const date = new Date(iso);
    return Number.isNaN(date.getTime()) ? iso : dateTimeFormat.format(date);
}
