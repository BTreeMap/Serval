import { useCallback, useEffect, useState } from "react";
import { api, setAuthToken, setDataPlaneUrl, type AuthMode, type Me, type OAuthFrontendConfig } from "./api";
import { AuthContext, type AuthState } from "./auth-context";
import { beginAuthorizationFlow, completeAuthorizationFlow, selectBearerToken } from "./oidc";

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
    const [oauthConfig, setOauthConfig] = useState<OAuthFrontendConfig | null>(null);
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
                setDataPlaneUrl(info.data_plane_url ?? null);
                setOauthConfig(info.oauth ?? null);
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

    const login = useCallback(
        async (token: string) => {
            localStorage.setItem(TOKEN_KEY, token);
            setAuthToken(token);
            await probe();
        },
        [probe],
    );

    const startOAuthLogin = useCallback(async () => {
        if (mode !== "oauth") {
            return;
        }
        if (!oauthConfig) {
            throw new Error("OAuth is not configured on this deployment.");
        }
        await beginAuthorizationFlow(oauthConfig);
    }, [mode, oauthConfig]);

    const completeOAuthLogin = useCallback(
        async (code: string, state: string) => {
            if (!oauthConfig) {
                throw new Error("OAuth is not configured on this deployment.");
            }
            const tokenResponse = await completeAuthorizationFlow({
                code,
                state,
                config: oauthConfig,
            });
            await login(selectBearerToken(tokenResponse));
        },
        [login, oauthConfig],
    );

    const signOut = useCallback(() => {
        localStorage.removeItem(TOKEN_KEY);
        setAuthToken(null);
        setMe(null);
    }, []);

    const value: AuthState = {
        me,
        mode,
        loading,
        oauthConfig,
        startOAuthLogin,
        completeOAuthLogin,
        signOut,
    };
    return <AuthContext value={value}>{children}</AuthContext>;
}
