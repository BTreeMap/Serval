import { useCallback, useEffect, useRef, useState } from "react";
import { LOADING, failure, success, valueOf, type RemoteData } from "./remote-data";
import { messageOf } from "./errors";

/** A keyed remote read, plus a way to re-run it. */
export interface RemoteQuery<T> {
    readonly state: RemoteData<T>;
    /** Re-run the read *without* discarding the value already on screen, so a
     *  post-mutation refetch updates in place instead of flashing a spinner.
     *
     *  The returned promise settles once the fresh result has been committed,
     *  so a mutation can await a consistent view before closing its editor —
     *  otherwise the editor would close over the *old* value for the frame or
     *  two until the refetch lands. */
    readonly refresh: () => Promise<void>;
}

/** One settled read, tagged with the key it answers. Storing the key alongside
 *  the result is what makes a stale answer unrepresentable: a settlement for
 *  another key is simply not this key's state. */
interface Settlement<T> {
    readonly key: string;
    readonly data: RemoteData<T>;
}

/** Resolve and clear everyone awaiting the next commit. */
function drain(waiters: { current: (() => void)[] }): void {
    const pending = waiters.current;
    waiters.current = [];
    for (const resolve of pending) {
        resolve();
    }
}

/** Run `load` whenever `key` changes, and expose the result as a {@link RemoteData}.
 *
 *  Two properties are structural rather than defensive:
 *
 *  - **`loading` is derived, never stored.** It is `settlement.key !== key` — the
 *    absence of an answer for the current question. Nothing sets it, so it cannot
 *    disagree with the data beside it, and there is no synchronous state write in
 *    an effect for `react-hooks/set-state-in-effect` to object to.
 *  - **A stale response cannot land.** The effect's cleanup aborts the previous
 *    request and every commit is gated on its own signal, so at most one request
 *    is live and the newest commit always wins. Switching snippets mid-flight can
 *    therefore never display the previous snippet's body under the new URL.
 *
 *  `load` is read through a ref so callers may pass an inline closure without
 *  re-triggering the request on every render; the request identity is `key`
 *  (plus an explicit `refresh`), which is the honest dependency. */
export function useRemoteQuery<T>(
    key: string,
    load: (signal: AbortSignal, isRevalidation: boolean) => Promise<T>,
): RemoteQuery<T> {
    const [settlement, setSettlement] = useState<Settlement<T> | null>(null);
    const [epoch, setEpoch] = useState(0);

    const loadRef = useRef(load);
    useEffect(() => {
        loadRef.current = load;
    });

    // Whether this key has already been answered once. It is the difference
    // between a first read — which may be served from a warmed prefetch — and a
    // revalidation, which must not be: a refetch after a create or a restore
    // exists precisely to observe that write, and a prefetch warmed seconds
    // earlier predates it. Deciding this here, rather than at each call site,
    // is what keeps the two callers from drifting apart.
    const answeredKey = useRef<string | null>(null);

    // Callers awaiting the next commit. Drained on every settlement, and on
    // unmount so no awaiting caller hangs forever.
    const waiters = useRef<(() => void)[]>([]);
    useEffect(() => () => drain(waiters), []);

    useEffect(() => {
        const controller = new AbortController();
        const { signal } = controller;
        loadRef.current(signal, answeredKey.current === key).then(
            (value) => {
                if (signal.aborted) {
                    return;
                }
                answeredKey.current = key;
                setSettlement({ key, data: success(value) });
                drain(waiters);
            },
            (error: unknown) => {
                if (signal.aborted) {
                    return;
                }
                answeredKey.current = key;
                // Keep the last good value for this key so a failed
                // revalidation shows an error *over* the data rather than
                // replacing it. `stale` is null iff no success preceded it.
                setSettlement((prev) => ({
                    key,
                    data: failure(
                        messageOf(error),
                        prev !== null && prev.key === key ? valueOf(prev.data) : null,
                    ),
                }));
                drain(waiters);
            },
        );
        return () => controller.abort();
    }, [key, epoch]);

    const refresh = useCallback(
        () =>
            new Promise<void>((resolve) => {
                waiters.current.push(resolve);
                setEpoch((previous) => previous + 1);
            }),
        [],
    );

    const state: RemoteData<T> =
        settlement !== null && settlement.key === key ? settlement.data : LOADING;

    return { state, refresh };
}
