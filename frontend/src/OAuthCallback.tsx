import { useEffect, useState } from "react";
import { Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { useAuth } from "./auth-context";
import { Banner, Card, Loading } from "./ui";

export function OAuthCallback() {
    const [searchParams] = useSearchParams();
    const navigate = useNavigate();
    const { mode, completeOAuthLogin } = useAuth();
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        const run = async () => {
            const providerError = searchParams.get("error");
            if (providerError) {
                setError(searchParams.get("error_description") || providerError);
                return;
            }

            const code = searchParams.get("code");
            const state = searchParams.get("state");
            if (!code || !state) {
                setError("Missing OAuth callback parameters.");
                return;
            }

            try {
                await completeOAuthLogin(code, state);
                navigate("/", { replace: true });
            } catch (cause) {
                setError(
                    cause instanceof Error ? cause.message : "Failed to complete OAuth sign-in.",
                );
            }
        };

        void run();
    }, [completeOAuthLogin, navigate, searchParams]);

    if (mode !== "oauth") {
        return <Navigate to="/" replace />;
    }

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