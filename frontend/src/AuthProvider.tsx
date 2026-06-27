import { useCallback, useEffect, useState } from "react";
import { api, setAuthToken, type AuthMode, type Me } from "./api";
import { AuthContext, type AuthState } from "./auth-context";

const TOKEN_KEY = "serval.token";

/** Provides authentication state, persisting the bearer token across reloads.
 *
 * When the backend runs with `AUTH_MODE=none` the `/api/me` probe succeeds
 * without a token (the dev superuser), so the dashboard is immediately usable.
 * Under `AUTH_MODE=cloudflare` the probe also succeeds with no token: Cloudflare
 * Access injects the identity header at the edge, so no token-paste step is
 * needed. The mode is fetched up front so the sign-in screen — shown only when
 * the probe fails — can present the right guidance. */
export function AuthProvider({ children }: { children: React.ReactNode }) {
    const [me, setMe] = useState<Me | null>(null);
    const [mode, setMode] = useState<AuthMode | null>(null);
    const [loading, setLoading] = useState(true);

    const probe = useCallback(async () => {
        setLoading(true);
        try {
            const [info, identity] = await Promise.all([
                api.authInfo().catch(() => null),
                api.me().catch(() => null),
            ]);
            if (info) {
                setMode(info.mode);
            }
            setMe(identity);
        } finally {
            setLoading(false);
        }
    }, []);

    useEffect(() => {
        const stored = localStorage.getItem(TOKEN_KEY);
        if (stored) {
            setAuthToken(stored);
        }
        // `probe` only updates state after an awaited request, so the renders
        // are not the synchronous cascade this rule guards against.
        // eslint-disable-next-line react-hooks/set-state-in-effect
        void probe();
    }, [probe]);

    const signIn = useCallback(
        async (token: string) => {
            localStorage.setItem(TOKEN_KEY, token);
            setAuthToken(token);
            await probe();
        },
        [probe],
    );

    const signOut = useCallback(() => {
        localStorage.removeItem(TOKEN_KEY);
        setAuthToken(null);
        setMe(null);
    }, []);

    const value: AuthState = { me, mode, loading, signIn, signOut };
    return <AuthContext value={value}>{children}</AuthContext>;
}
