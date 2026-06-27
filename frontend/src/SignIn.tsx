import { useState } from "react";
import { useAuth } from "./auth-context";
import { Button, Card, ErrorBanner } from "./ui";

/** A minimal token entry gate, shown only when the backend enforces auth and no
 *  valid token is present. With `AUTH_MODE=none` this screen is never reached. */
export function SignIn() {
    const { signIn } = useAuth();
    const [token, setToken] = useState("");
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);

    const submit = async (event: React.FormEvent) => {
        event.preventDefault();
        if (!token.trim()) {
            return;
        }
        setBusy(true);
        setError(null);
        try {
            await signIn(token.trim());
        } catch {
            setError("That token was not accepted. Please try again.");
        } finally {
            setBusy(false);
        }
    };

    return (
        <div className="flex min-h-full items-center justify-center bg-canvas px-6">
            <Card className="w-full max-w-md">
                <h1 className="text-xl font-semibold">Sign in to Serval</h1>
                <p className="mt-1 text-sm text-ink-soft">
                    Paste a bearer token issued by your identity provider.
                </p>
                <form onSubmit={(e) => void submit(e)} className="mt-6 space-y-4">
                    <textarea
                        value={token}
                        onChange={(e) => setToken(e.target.value)}
                        placeholder="eyJhbGciOi…"
                        rows={4}
                        className="w-full resize-none rounded-lg border border-line bg-canvas px-3 py-2 font-mono text-xs text-ink focus:border-wisteria focus:outline-none"
                    />
                    {error && <ErrorBanner message={error} />}
                    <Button type="submit" disabled={busy} className="w-full">
                        {busy ? "Verifying…" : "Continue"}
                    </Button>
                </form>
            </Card>
        </div>
    );
}
