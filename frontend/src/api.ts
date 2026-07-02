// The single gateway between the dashboard and the Control Plane. Every network
// call funnels through `request` so that auth, error shaping, and JSON handling
// live in exactly one place — components never touch `fetch` directly.

/** A snippet route as returned by create/update. */
export interface SnippetResponse {
    id: string;
    content_type: string;
    title?: string | null;
    description?: string | null;
    owner_id: string | null;
}

/** Hard cap on rows requested or rendered per page, matching the backend's
 *  `MAX_PAGE_LIMIT`. Neither the client nor the UI should ever ask for more. */
export const MAX_PAGE_LIMIT = 50;

/** One immutable entry in a route's append-only version ledger.
 *
 *  `version_number` and `is_current` are computed server-side from the exact
 *  ledger total captured when pagination began, so a badge like `v12` stays
 *  correct regardless of which page happens to be loaded in the browser. */
export interface HistoryItem {
    target_hash: string;
    editor_id: string;
    changed_at: string;
    version_number: number;
    is_current: boolean;
}

/** The content of one historical version, fetched for previewing. */
export interface VersionContent {
    target_hash: string;
    content: string;
}

/** A route plus the newest page of its version history. `history_count` is
 *  always the exact, unpaginated ledger size; `history` holds only the
 *  newest `history_limit` entries. Fetch older entries via
 *  `api.listSnippetHistory`. */
export interface SnippetDetail {
    id: string;
    content_type: string;
    title?: string | null;
    description?: string | null;
    owner_id: string | null;
    history_count: number;
    history: HistoryItem[];
    history_next_cursor: string | null;
    history_limit: number;
}

/** One page of the owner's snippet listing. `next_cursor` is `null` once the
 *  caller has reached the end of the list. */
export interface SnippetListResponse {
    snippets: SnippetSummary[];
    next_cursor: string | null;
    limit: number;
}

/** One page of a route's version history, for "Load older versions". */
export interface HistoryPageResponse {
    history: HistoryItem[];
    next_cursor: string | null;
    limit: number;
}

/** Shared `?limit=&cursor=` pagination request params. */
export interface PageParams {
    limit?: number;
    cursor?: string | null;
}

/** A compact route listing entry for the dashboard index. */
export interface SnippetSummary {
    id: string;
    content_type: string;
    title?: string | null;
    description?: string | null;
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

/** Public bootstrap metadata, fetched before the caller is authenticated so the
 *  sign-in screen can present the right flow and the dashboard knows where the
 *  Data Plane lives. */
export interface AuthInfo {
    mode: AuthMode;
    /** Public base URL of the Data Plane, e.g. `https://cdn.example.com`. Null
     *  when the backend leaves it unconfigured. */
    data_plane_url?: string | null;
    /** Frontend-safe OAuth bootstrap settings for browser-managed login. */
    oauth?: OAuthFrontendConfig | null;
}

/** Public OAuth bootstrap settings exposed by the Control Plane. */
export interface OAuthFrontendConfig {
    issuer_url: string;
    client_id: string;
    scopes: string;
    redirect_uri: string;
}

/** Minimal OIDC discovery document fields required for PKCE login. */
export interface OidcDiscoveryResponse {
    authorization_endpoint: string;
    token_endpoint: string;
}

/** Token response from the OAuth provider's token endpoint. */
export interface OidcTokenResponse {
    access_token: string;
    token_type: string;
    expires_in?: number;
    id_token?: string;
    scope?: string;
    refresh_token?: string;
}

/** Payload for minting a new snippet. */
export interface CreateRequest {
    content: string;
    content_type?: string;
    title?: string;
    description?: string;
}

/** Payload for a partial snippet update: repoint at new `content`, change the
 *  stored `content_type`, update `title`/`description`, or any combination.
 *  At least one field must be present. Send an empty string for `title` or
 *  `description` to clear them. */
export interface UpdateRequest {
    content?: string;
    content_type?: string;
    title?: string;
    description?: string;
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

/** The Data Plane base URL advertised by the backend at bootstrap, when set. */
let dataPlaneBaseUrl: string | null = null;

/** Record the backend-advertised Data Plane base URL (call once at startup). */
export function setDataPlaneUrl(url: string | null): void {
    const trimmed = url?.trim().replace(/\/+$/, "");
    dataPlaneBaseUrl = trimmed ? trimmed : null;
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

/** Coerce all carriage returns and CRLF pairs to a standard LF newline. */
function normalizeNewlines(text: string): string {
    return text.replace(/\r\n|\r/g, "\n");
}

/** Build a `?limit=&cursor=` query string from pagination params, omitting
 *  absent fields entirely rather than sending empty values. */
function pageQuery(params: PageParams): string {
    const search = new URLSearchParams();
    if (params.limit !== undefined) {
        search.set("limit", String(params.limit));
    }
    if (params.cursor) {
        search.set("cursor", params.cursor);
    }
    const query = search.toString();
    return query ? `?${query}` : "";
}

export const api = {
    authInfo(): Promise<AuthInfo> {
        return request<AuthInfo>("/api/auth-info");
    },

    async getOidcDiscovery(issuerUrl: string): Promise<OidcDiscoveryResponse> {
        const discoveryUrl = `${issuerUrl.replace(/\/$/, "")}/.well-known/openid-configuration`;
        const response = await fetch(discoveryUrl);
        if (!response.ok) {
            throw new ApiError(response.status, await extractError(response));
        }
        return (await response.json()) as OidcDiscoveryResponse;
    },

    async exchangeOidcCode(params: {
        tokenEndpoint: string;
        code: string;
        clientId: string;
        redirectUri: string;
        codeVerifier: string;
    }): Promise<OidcTokenResponse> {
        const body = new URLSearchParams({
            grant_type: "authorization_code",
            code: params.code,
            client_id: params.clientId,
            redirect_uri: params.redirectUri,
            code_verifier: params.codeVerifier,
        });

        const response = await fetch(params.tokenEndpoint, {
            method: "POST",
            headers: { "Content-Type": "application/x-www-form-urlencoded" },
            body,
        });
        if (!response.ok) {
            throw new ApiError(response.status, await extractError(response));
        }
        return (await response.json()) as OidcTokenResponse;
    },

    me(): Promise<Me> {
        return request<Me>("/api/me");
    },

    listSnippets(params: PageParams = {}): Promise<SnippetListResponse> {
        return request<SnippetListResponse>(`/api/snippets${pageQuery(params)}`);
    },

    createSnippet(payload: CreateRequest): Promise<SnippetResponse> {
        const normalizedPayload = {
            ...payload,
            content: normalizeNewlines(payload.content),
        };
        return request<SnippetResponse>("/api/snippets", {
            method: "POST",
            json: normalizedPayload,
        });
    },

    getSnippet(id: string, params: PageParams = {}): Promise<SnippetDetail> {
        return request<SnippetDetail>(
            `/api/snippets/${encodeURIComponent(id)}${pageQuery(params)}`,
        );
    },

    listSnippetHistory(id: string, params: PageParams = {}): Promise<HistoryPageResponse> {
        return request<HistoryPageResponse>(
            `/api/snippets/${encodeURIComponent(id)}/history${pageQuery(params)}`,
        );
    },

    updateSnippet(id: string, update: UpdateRequest): Promise<SnippetResponse> {
        const normalizedUpdate = { ...update };
        if (normalizedUpdate.content !== undefined) {
            normalizedUpdate.content = normalizeNewlines(normalizedUpdate.content);
        }
        return request<SnippetResponse>(`/api/snippets/${encodeURIComponent(id)}`, {
            method: "PATCH",
            json: normalizedUpdate,
        });
    },

    getVersion(id: string, hash: string): Promise<VersionContent> {
        return request<VersionContent>(
            `/api/snippets/${encodeURIComponent(id)}/versions/${encodeURIComponent(hash)}`,
        );
    },

    restoreVersion(id: string, hash: string): Promise<SnippetResponse> {
        return request<SnippetResponse>(
            `/api/snippets/${encodeURIComponent(id)}/restore`,
            {
                method: "POST",
                json: { target_hash: hash },
            },
        );
    },
};

/** Build the public Data Plane delivery URL for a snippet id.
 *
 * The Data Plane usually lives on a different domain than the dashboard, so the
 * base is resolved in priority order: the backend-advertised URL (set at
 * bootstrap), then the build-time `VITE_DATA_PLANE_URL`, and finally a
 * best-effort guess of `:3000` on the dashboard's own hostname for local dev. */
export function deliveryUrl(id: string): string {
    const buildTime = import.meta.env.VITE_DATA_PLANE_URL?.trim().replace(/\/+$/, "");
    const base =
        dataPlaneBaseUrl ??
        (buildTime ? buildTime : null) ??
        `${window.location.protocol}//${window.location.hostname}:3000`;
    return new URL(encodeURIComponent(id), `${base}/`).toString();
}
