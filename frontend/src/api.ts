// The single gateway between the dashboard and the Control Plane. Every network
// call funnels through `request` so that auth, error shaping, and JSON handling
// live in exactly one place — components never touch `fetch` directly.

/** A snippet route as returned by create/update. */
export interface SnippetResponse {
    id: string;
    immutable: boolean;
    content_type: string;
    owner_id: string | null;
}

/** One immutable entry in a route's append-only version ledger. */
export interface HistoryItem {
    target_hash: string;
    editor_id: string;
    changed_at: string;
}

/** A route plus its full version history. */
export interface SnippetDetail {
    id: string;
    immutable: boolean;
    content_type: string;
    owner_id: string | null;
    history_count: number;
    history: HistoryItem[];
}

/** A compact route listing entry for the dashboard index. */
export interface SnippetSummary {
    id: string;
    immutable: boolean;
    content_type: string;
    owner_id: string | null;
    updated_at: string;
}

/** The authenticated caller and their locally-managed admin role. */
export interface Me {
    user_id: string;
    is_admin: boolean;
}

/** The authentication mode the backend enforces. */
export type AuthMode = "none" | "oauth" | "cloudflare";

/** Public auth metadata, fetched before the caller is authenticated so the
 *  sign-in screen can present the right flow. */
export interface AuthInfo {
    mode: AuthMode;
}

/** Payload for minting a new snippet. */
export interface CreateRequest {
    content: string;
    content_type?: string;
    immutable: boolean;
}

/** A typed error carrying the HTTP status for caller-side branching. */
export class ApiError extends Error {
    readonly status: number;

    constructor(status: number, message: string) {
        super(message);
        this.name = "ApiError";
        this.status = status;
    }
}

/** The bearer token used for authenticated requests, when present. */
let authToken: string | null = null;

/** Set (or clear) the bearer token sent with every subsequent request. */
export function setAuthToken(token: string | null): void {
    authToken = token;
}

async function request<T>(
    path: string,
    init: RequestInit & { json?: unknown } = {},
): Promise<T> {
    const { json, headers, ...rest } = init;
    const finalHeaders = new Headers(headers);

    if (json !== undefined) {
        finalHeaders.set("Content-Type", "application/json");
    }
    if (authToken) {
        finalHeaders.set("Authorization", `Bearer ${authToken}`);
    }

    const response = await fetch(path, {
        ...rest,
        headers: finalHeaders,
        body: json !== undefined ? JSON.stringify(json) : rest.body,
    });

    if (!response.ok) {
        throw new ApiError(response.status, await extractError(response));
    }

    if (response.status === 204) {
        return undefined as T;
    }
    return (await response.json()) as T;
}

/** Pull a human-readable message out of an error response body. */
async function extractError(response: Response): Promise<string> {
    try {
        const body = (await response.json()) as { error?: string };
        if (body.error) {
            return body.error;
        }
    } catch {
        // Fall through to the status text below.
    }
    return response.statusText || `request failed with status ${response.status}`;
}

export const api = {
    authInfo(): Promise<AuthInfo> {
        return request<AuthInfo>("/api/auth-info");
    },

    me(): Promise<Me> {
        return request<Me>("/api/me");
    },

    listSnippets(): Promise<SnippetSummary[]> {
        return request<SnippetSummary[]>("/api/snippets");
    },

    createSnippet(payload: CreateRequest): Promise<SnippetResponse> {
        return request<SnippetResponse>("/api/snippets", {
            method: "POST",
            json: payload,
        });
    },

    getSnippet(id: string): Promise<SnippetDetail> {
        return request<SnippetDetail>(`/api/snippets/${encodeURIComponent(id)}`);
    },

    updateSnippet(id: string, content: string): Promise<SnippetResponse> {
        return request<SnippetResponse>(`/api/snippets/${encodeURIComponent(id)}`, {
            method: "PATCH",
            json: { content },
        });
    },
};

/** Build the public Data Plane delivery URL for a snippet id. */
export function deliveryUrl(id: string): string {
    return `${window.location.protocol}//${window.location.hostname}:3000/${id}`;
}
