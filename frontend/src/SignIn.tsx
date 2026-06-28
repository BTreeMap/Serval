import { useState } from "react";
import { useAuth } from "./auth-context";
import { Banner, Button, Card } from "./ui";

/** The sign-in gate, shown only when the backend enforces auth and the initial
 *  identity probe failed. The affordance depends on the backend's auth mode:
 *  Cloudflare Access authenticates at the edge (nothing to paste), while a
 *  generic OAuth provider expects a bearer token. */
export function SignIn() {
    const { mode } = useAuth();

    if (mode === "cloudflare") {
        return <CloudflareSignIn />;
    }
    return <OAuthSignIn />;
}

/** Cloudflare Access injects identity at the edge, so there is no token to
 *  paste. If we still reached this screen the Access session is missing or
 *  expired; reloading re-runs the Access challenge. */
function CloudflareSignIn() {
    return (
        <div className="flex min-h-full items-center justify-center bg-canvas px-6">
            <Card className="w-full max-w-md">
                <h1 className="text-xl font-semibold">Sign in to Serval</h1>
                <p className="mt-1 text-sm text-ink-soft">
                    This deployment is protected by Cloudflare Access. Your identity is
                    verified automatically at the edge — there is no token to enter.
                </p>
                <p className="mt-3 text-sm text-ink-soft">
                    If you are seeing this, your Access session is missing or has expired.
                    Reload to complete the Cloudflare sign-in.
                </p>
                <Button onClick={() => window.location.reload()} type="button" className="mt-6 w-full">
                    Reload
                </Button>
            </Card>
        </div>
    );
}

/** The browser-driven OAuth entry point for generic OIDC providers. */
function OAuthSignIn() {
    const { startOAuthLogin } = useAuth();
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);

    const submit = async (event: React.FormEvent) => {
        event.preventDefault();
        setBusy(true);
        setError(null);
        try {
            await startOAuthLogin();
        } catch (cause) {
            setBusy(false);
            setError(cause instanceof Error ? cause.message : "Failed to start OAuth sign-in.");
        }
    };

    return (
        <div className="flex min-h-full items-center justify-center bg-canvas px-6">
            <Card className="w-full max-w-md">
                <h1 className="text-xl font-semibold">Sign in to Serval</h1>
                <p className="mt-1 text-sm text-ink-soft">
                    Continue with your identity provider to sign in securely.
                </p>
                <form onSubmit={(e) => void submit(e)} className="mt-6 space-y-4">
                    {error && <Banner tone="error">{error}</Banner>}
                    <Button type="submit" loading={busy} className="w-full">
                        {busy ? "Redirecting…" : "Sign in with OAuth"}
                    </Button>
                </form>
            </Card>
        </div>
    );
}
