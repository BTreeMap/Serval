import { createContext, useContext } from "react";
import type { AuthMode, Me, OAuthFrontendConfig } from "./api";

/** Shared authentication state exposed to the whole app. */
export interface AuthState {
    /** The authenticated caller, or `null` while loading / when signed out. */
    me: Me | null;
    /** The backend's auth mode, or `null` until the initial probe resolves. */
    mode: AuthMode | null;
    /** True while the initial identity probe is in flight. */
    loading: boolean;
    /** Frontend-safe OAuth bootstrap settings, when oauth mode is enabled. */
    oauthConfig: OAuthFrontendConfig | null;
    /** Begin the browser-driven OAuth PKCE flow. */
    startOAuthLogin: () => Promise<void>;
    /** Complete the OAuth callback by exchanging the auth code for tokens. */
    completeOAuthLogin: (code: string, state: string) => Promise<void>;
    /** Clear the active token and identity. */
    signOut: () => void;
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
