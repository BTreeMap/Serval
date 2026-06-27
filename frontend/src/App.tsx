import { Navigate, Route, Routes } from "react-router-dom";
import { AuthProvider } from "./AuthProvider";
import { useAuth } from "./auth-context";
import { Dashboard } from "./Dashboard";
import { Layout } from "./Layout";
import { SignIn } from "./SignIn";
import { SnippetDetail } from "./SnippetDetail";

/** The application root: wires the auth provider around the routed shell. */
export function App() {
  return (
    <AuthProvider>
      <Gate />
    </AuthProvider>
  );
}

/** Gate the app behind authentication. With `AUTH_MODE=none` the identity probe
 *  succeeds immediately, so the sign-in screen is skipped entirely. */
function Gate() {
  const { me, loading } = useAuth();

  if (loading) {
    return (
      <div className="flex min-h-full items-center justify-center bg-canvas text-ink-soft">
        Loading…
      </div>
    );
  }

  if (!me) {
    return <SignIn />;
  }

  return (
    <Layout>
      <Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/s/:id" element={<SnippetDetail />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </Layout>
  );
}
