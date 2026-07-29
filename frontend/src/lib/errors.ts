// Imports inside `lib/` carry the `.ts` extension so this core runs unbundled
// under `node --test`. Nothing here may import from outside `lib/`.
import { ApiError } from "./api-error.ts";

/** The user-facing message for an unknown thrown value.
 *
 *  Only an `ApiError` carries a message the Control Plane intended a human to
 *  read; anything else (a `TypeError` from a dropped connection, a parse
 *  failure) would leak runtime vocabulary into the UI, so it collapses to one
 *  neutral sentence. This is the single definition — previously three call
 *  sites each carried their own copy and one of them differed. */
export function messageOf(err: unknown): string {
    return err instanceof ApiError ? err.message : "Something went wrong. Please try again.";
}
