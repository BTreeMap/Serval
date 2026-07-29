import { createContext, useContext } from "react";
import type { AuthMode, Me, OAuthFrontendConfig } from "./api";

/** Everything one identity probe establishes, committed as a single value.
 *
 *  These three facts arrive together from one round trip, so they are a product
 *  and are written atomically — as separate state cells they could be observed
 *  half-updated, with an identity from the new probe beside a mode from the old. */
export interface Session {
    /** The authenticated caller, or `null` when signed out. */
    readonly me: Me | null;
    /** The backend's auth mode, or `null` if the bootstrap read never succeeded. */
    readonly mode: AuthMode | null;
    /** Frontend-safe OAuth bootstrap settings, when oauth mode is enabled. */
    readonly oauthConfig: OAuthFrontendConfig | null;
}

/** Shared authentication state exposed to the whole app. */
export interface AuthState extends Session {
    /** True while the newest identity probe is still in flight. Derived from
     *  whether that probe has settled, never stored alongside the session. */
    readonly loading: boolean;
    /** Begin the browser-driven OAuth PKCE flow. */
    readonly startOAuthLogin: () => Promise<void>;
    /** Complete the OAuth callback by exchanging the auth code for tokens. */
    readonly completeOAuthLogin: (code: string, state: string) => Promise<void>;
    /** Clear the active token and identity. */
    readonly signOut: () => void;
}

export const AuthContext = createContext<AuthState | null>(null);

/** Access the ambient auth state; throws if used outside the provider. */
export function useAuth(): AuthState {
    const ctx = useContext(AuthContext);
    if (!ctx) {
        throw new Error("useAuth must be used within an AuthProvider");
    }
    return ctx;
}
