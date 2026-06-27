import { createContext, useContext } from "react";
import type { Me } from "./api";

/** Shared authentication state exposed to the whole app. */
export interface AuthState {
    /** The authenticated caller, or `null` while loading / when signed out. */
    me: Me | null;
    /** True while the initial identity probe is in flight. */
    loading: boolean;
    /** Replace the active bearer token and re-probe identity. */
    signIn: (token: string) => Promise<void>;
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
