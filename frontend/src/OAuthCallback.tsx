import { useEffect, useState } from "react";
import { Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { useAuth } from "./auth-context";
import { Banner, Card, Loading } from "./ui";

/** The OAuth redirect target: exchanges the authorization code for a token and
 *  returns to the dashboard.
 *
 *  Both branches below wait for the bootstrap probe to settle. Redirecting on
 *  `mode !== "oauth"` while `mode` is still `null` fired on the very first
 *  render — before any probe could have answered — so this screen bounced to
 *  `/` every time and neither its progress nor its failure state was reachable.
 *  Starting the exchange early had the same root cause: `completeOAuthLogin`
 *  needs the OAuth config the probe fetches, so it failed with a misleading
 *  "OAuth is not configured" before the config had had a chance to arrive. */
export function OAuthCallback() {
    const [searchParams] = useSearchParams();
    const navigate = useNavigate();
    const { mode, loading, completeOAuthLogin } = useAuth();
    const [exchangeError, setExchangeError] = useState<string | null>(null);

    // Derived from the URL during render rather than pushed into state by an
    // effect: these are a pure function of the query string, and storing them
    // would be a second copy that can disagree with it.
    const providerError = searchParams.get("error");
    const code = searchParams.get("code");
    const state = searchParams.get("state");
    const paramError = providerError
        ? (searchParams.get("error_description") || providerError)
        : !code || !state
          ? "Missing OAuth callback parameters."
          : null;

    useEffect(() => {
        if (loading || mode !== "oauth" || paramError || !code || !state) {
            return;
        }
        let cancelled = false;
        void completeOAuthLogin(code, state).then(
            () => {
                if (!cancelled) {
                    navigate("/", { replace: true });
                }
            },
            (cause: unknown) => {
                if (!cancelled) {
                    setExchangeError(
                        cause instanceof Error
                            ? cause.message
                            : "Failed to complete OAuth sign-in.",
                    );
                }
            },
        );
        return () => {
            cancelled = true;
        };
    }, [loading, mode, paramError, code, state, completeOAuthLogin, navigate]);

    if (!loading && mode !== "oauth") {
        return <Navigate to="/" replace />;
    }

    const error = paramError ?? exchangeError;

    return (
        <div className="flex min-h-full items-center justify-center bg-canvas px-6">
            <Card className="w-full max-w-md">
                {error ? (
                    <>
                        <h1 className="text-xl font-semibold">Sign-in failed</h1>
                        <div className="mt-4">
                            <Banner tone="error">{error}</Banner>
                        </div>
                    </>
                ) : (
                    <div className="flex flex-col items-center gap-3 py-4 text-center">
                        <Loading />
                        <p className="text-sm text-ink-soft">Completing sign-in…</p>
                    </div>
                )}
            </Card>
        </div>
    );
}
