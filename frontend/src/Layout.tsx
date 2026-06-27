import { Link } from "react-router-dom";
import { useAuth } from "./auth-context";
import { Badge, Button } from "./ui";

/** The app shell: top bar with identity and sign-out, plus routed content. */
export function Layout({ children }: { children: React.ReactNode }) {
    const { me, signOut } = useAuth();

    return (
        <div className="min-h-full bg-slate-950 text-slate-100">
            <header className="border-b border-slate-800 bg-slate-900/50 backdrop-blur">
                <div className="mx-auto flex max-w-5xl items-center justify-between px-6 py-4">
                    <Link to="/" className="flex items-center gap-2">
                        <img src="/favicon.svg" alt="" className="h-7 w-7" />
                        <span className="text-lg font-semibold tracking-tight">Serval</span>
                    </Link>
                    {me && (
                        <div className="flex items-center gap-3">
                            <span className="text-sm text-slate-400">{me.user_id}</span>
                            {me.is_admin && <Badge tone="sky">admin</Badge>}
                            <Button variant="ghost" onClick={signOut}>
                                Sign out
                            </Button>
                        </div>
                    )}
                </div>
            </header>
            <main className="mx-auto max-w-5xl px-6 py-8">{children}</main>
        </div>
    );
}
