import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, setAuthToken, setDataPlaneUrl } from "./api";
import { AuthContext, type AuthState, type Session } from "./auth-context";
import { beginAuthorizationFlow, completeAuthorizationFlow, selectBearerToken } from "./oidc";

const TOKEN_KEY = "serval.token";

const EMPTY_SESSION: Session = Object.freeze({ me: null, mode: null, oauthConfig: null });

/** Provides authentication state, persisting the bearer token across reloads.
 *
 * When the backend runs with `AUTH_MODE=none` the `/api/me` probe succeeds
 * without a token (the dev superuser), so the dashboard is immediately usable.
 * Under `AUTH_MODE=cloudflare` the probe also succeeds with no token: Cloudflare
 * Access injects the identity header at the edge, so no token-paste step is
 * needed. The mode is fetched up front so the sign-in screen — shown only when
 * the probe fails — can present the right guidance.
 *
 * The probe's results used to be four `useState` cells written one after
 * another, so a render could observe half a probe. They are now one value
 * committed once, and `loading` is *derived* from whether the newest probe has
 * settled rather than stored beside it — which is also why this file no longer
 * needs to silence `react-hooks/set-state-in-effect`. */
export function AuthProvider({ children }: { children: React.ReactNode }) {
    const [epoch, setEpoch] = useState(0);
    const [settled, setSettled] = useState<{ epoch: number; session: Session } | null>(null);

    // Callers awaiting the probe their own `login` triggered.
    const waiters = useRef<(() => void)[]>([]);

    // Restore the persisted bearer token before the first probe. Effects run in
    // declaration order, so this lands before the probe effect below and the
    // identity request carries the token.
    useEffect(() => {
        const stored = localStorage.getItem(TOKEN_KEY);
        if (stored) {
            setAuthToken(stored);
        }
    }, []);

    useEffect(() => {
        const controller = new AbortController();
        const { signal } = controller;
        // Independent reads, so they go out together: the bootstrap metadata
        // does not depend on the identity. Each absorbs its own failure — an
        // anonymous caller failing `/api/me` is the normal signed-out path, not
        // an error to propagate.
        void Promise.all([
            api.authInfo(signal).catch(() => null),
            api.me(signal).catch(() => null),
        ]).then(([info, identity]) => {
            if (signal.aborted) {
                return;
            }
            if (info) {
                setDataPlaneUrl(info.data_plane_url ?? null);
            }
            setSettled((prev) => ({
                epoch,
                session: {
                    me: identity,
                    // A failed `authInfo` leaves the previously known mode and
                    // OAuth config in place rather than blanking them.
                    mode: info ? info.mode : (prev?.session.mode ?? null),
                    oauthConfig: info
                        ? (info.oauth ?? null)
                        : (prev?.session.oauthConfig ?? null),
                },
            }));
            const pending = waiters.current;
            waiters.current = [];
            for (const resolve of pending) {
                resolve();
            }
        });
        return () => controller.abort();
    }, [epoch]);

    /** Re-run the probe; resolves once the fresh result has been committed. */
    const probe = useCallback(
        () =>
            new Promise<void>((resolve) => {
                waiters.current.push(resolve);
                setEpoch((previous) => previous + 1);
            }),
        [],
    );

    const login = useCallback(
        async (token: string) => {
            localStorage.setItem(TOKEN_KEY, token);
            setAuthToken(token);
            await probe();
        },
        [probe],
    );

    const session = settled?.session ?? EMPTY_SESSION;
    const loading = settled === null || settled.epoch !== epoch;
    const { me, mode, oauthConfig } = session;

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
        setSettled((prev) =>
            prev === null ? prev : { ...prev, session: { ...prev.session, me: null } },
        );
    }, []);

    // Memoized: the context value is the identity every consumer compares
    // against, so rebuilding it on each render re-rendered every consumer
    // whether or not the session had actually changed.
    const value = useMemo<AuthState>(
        () => ({
            me,
            mode,
            oauthConfig,
            loading,
            startOAuthLogin,
            completeOAuthLogin,
            signOut,
        }),
        [me, mode, oauthConfig, loading, startOAuthLogin, completeOAuthLogin, signOut],
    );

    return <AuthContext value={value}>{children}</AuthContext>;
}
