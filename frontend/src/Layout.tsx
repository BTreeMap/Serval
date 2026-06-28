import { Link } from "react-router-dom";
import { useAuth } from "./auth-context";
import { Badge, Button, Icons } from "./ui";

/** The app shell: top bar with identity and sign-out, plus routed content. */
export function Layout({ children }: { children: React.ReactNode }) {
    const { me, mode, signOut } = useAuth();

    return (
        <div className="min-h-full bg-canvas text-ink">
            <header className="sticky top-0 z-20 border-b border-line bg-surface/70 backdrop-blur">
                <div className="mx-auto flex max-w-5xl flex-wrap items-center justify-between gap-x-4 gap-y-2 px-4 py-3 sm:px-6 sm:py-4 lg:px-8 lg:py-5">
                    <Link
                        to="/"
                        className="flex items-center gap-2 rounded-lg focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50"
                    >
                        <img src="/favicon.svg" alt="" className="h-7 w-7" />
                        <span className="text-lg font-semibold tracking-tight">Serval</span>
                    </Link>
                    {me && (
                        <div className="flex min-w-0 items-center gap-2 sm:gap-3">
                            <span className="max-w-40 truncate text-sm text-ink-soft sm:max-w-xs md:max-w-sm">
                                {me.user_id}
                            </span>
                            {me.is_admin && <Badge tone="wisteria">admin</Badge>}
                            {mode === "oauth" && (
                                <Button variant="ghost" size="sm" onClick={signOut}>
                                    <Icons.LogOut className="h-4 w-4" aria-hidden />
                                    <span className="hidden sm:inline">Sign out</span>
                                </Button>
                            )}
                        </div>
                    )}
                </div>
            </header>
            <main className="mx-auto max-w-5xl px-4 py-6 sm:px-6 sm:py-8 lg:px-8 lg:py-10">{children}</main>
        </div>
    );
}
