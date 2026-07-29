// The single vocabulary for "a value fetched over the network".
//
// Every view that loads something used to carry three independent cells —
// `data`, `loading`, `error` — whose 2^3 combinations include states no correct
// flow can reach (loading *and* failed; data present *and* still loading with no
// way to tell whether the data is the current key's). This closed union admits
// exactly the reachable ones, so rendering is a total function over three cases
// rather than a chain of guards over eight.

/** The lifecycle of one remote value.
 *
 *  `failure.stale` carries the most recent successful value for the *same*
 *  request key, when there is one. That is what lets a list stay on screen
 *  under an error banner after a failed revalidation — the alternative
 *  (a separate "last good value" cell) reintroduces the very product of
 *  independent cells this union exists to remove. */
export type RemoteData<T> =
    | { readonly tag: "loading" }
    | { readonly tag: "success"; readonly value: T }
    | { readonly tag: "failure"; readonly message: string; readonly stale: T | null };

/** The sole `loading` value.
 *
 *  Hoisted rather than constructed per render: a fresh object each render is
 *  referentially distinct, which silently defeats every downstream `useMemo`
 *  and `memo` keyed on the state. `loading` carries no payload, so one shared
 *  frozen value is observationally identical to a fresh one. */
export const LOADING: RemoteData<never> = Object.freeze({ tag: "loading" as const });

export const success = <T,>(value: T): RemoteData<T> => ({ tag: "success", value });

export const failure = <T,>(message: string, stale: T | null = null): RemoteData<T> => ({
    tag: "failure",
    message,
    stale,
});

/** Total eliminator: every variant must be handled, so adding a variant later
 *  is a compile error at each call site rather than a silently missing branch. */
export function foldRemote<T, R>(
    data: RemoteData<T>,
    cases: {
        readonly loading: () => R;
        readonly success: (value: T) => R;
        readonly failure: (message: string, stale: T | null) => R;
    },
): R {
    switch (data.tag) {
        case "loading":
            return cases.loading();
        case "success":
            return cases.success(data.value);
        case "failure":
            return cases.failure(data.message, data.stale);
        default:
            return assertNever(data);
    }
}

/** The value if one is known, else `null` — a success's value, or the stale
 *  value a failure is still holding. Total, and the only place the "keep the
 *  last list visible under an error" rule is written down. */
export function valueOf<T>(data: RemoteData<T>): T | null {
    switch (data.tag) {
        case "loading":
            return null;
        case "success":
            return data.value;
        case "failure":
            return data.stale;
        default:
            return assertNever(data);
    }
}

/** The message if this is a failure, else `null`. */
export function messageOfRemote<T>(data: RemoteData<T>): string | null {
    return data.tag === "failure" ? data.message : null;
}

/** Proof that a `switch` is exhaustive. Reachable only if a variant was added
 *  to the union without extending every eliminator, which the compiler rejects
 *  first; the throw exists for a value that crossed an untyped boundary. */
export function assertNever(value: never): never {
    throw new TypeError(`unexpected variant: ${JSON.stringify(value)}`);
}
