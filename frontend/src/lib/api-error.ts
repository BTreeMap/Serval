/** A typed error carrying the HTTP status for caller-side branching.
 *
 *  Lives apart from the API client so that the pure, framework-free core can
 *  recognise it without pulling in the client's build-time configuration. */
export class ApiError extends Error {
    readonly status: number;

    constructor(status: number, message: string) {
        super(message);
        this.name = "ApiError";
        this.status = status;
    }
}
