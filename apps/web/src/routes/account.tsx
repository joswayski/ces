import { createFileRoute } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { setResponseHeader } from "@tanstack/react-start/server";
import type { Account } from "../server/accountApi";

const loadAccount = createServerFn({ method: "GET" }).handler(async () => {
  setResponseHeader("Cache-Control", "no-store");
  const { currentAccount } = await import("../server/account");
  const response = await currentAccount();
  return {
    status: response.status,
    account: response.ok ? await response.json() as Account : null,
  };
});

export const Route = createFileRoute("/account")({
  validateSearch: (search: Record<string, unknown>) => ({
    error: search.error === "sign-in" ? "sign-in" : undefined,
  }),
  loader: () => loadAccount(),
  head: () => ({ meta: [{ title: "Account — Captures" }, { name: "robots", content: "noindex" }] }),
  component: AccountPage,
});

function AccountPage() {
  const { status, account } = Route.useLoaderData();
  const { error } = Route.useSearch();
  return (
    <main className="mx-auto flex min-h-dvh w-full max-w-xl flex-col justify-center px-6 py-12">
      <a href="/" className="mb-10 text-sm text-ink-muted underline-offset-4 hover:underline">← Captures</a>
      <p className="text-sm text-ink-soft">Optional account</p>
      <h1 className="mt-2 text-3xl font-medium tracking-tight text-ink">Your Captures account</h1>
      <p className="mt-4 text-sm leading-relaxed text-ink-muted">
        Screenshots, GIFs, and recordings work without an account. Accounts are
        being prepared for future cloud features; uploads and sharing are not available yet.
      </p>
      <section aria-label="Account status" className="mt-8 rounded-xl border border-border bg-surface p-6">
        {account ? (
          <>
            <h2 className="font-medium text-ink">You’re signed in</h2>
            <p className="mt-3 break-all text-sm text-ink-muted">{account.email}</p>
            <p className="mt-2 text-xs text-ink-soft">{account.emailVerified ? "Email verified" : "Email not verified"}</p>
            <form action="/api/auth/sign-out" method="post" className="mt-6">
              <button type="submit" className="inline-chip px-3 py-2">Sign out</button>
            </form>
          </>
        ) : status === 401 ? (
          <>
            <h2 className="font-medium text-ink">Sign in or create an account</h2>
            <p className="mt-3 text-sm leading-relaxed text-ink-muted">We’ll email you a one-time code. No password needed.</p>
            {error && <p role="alert" className="mt-3 text-sm text-ink-muted">Sign-in didn’t finish. Please try again.</p>}
            <a href="/api/auth/sign-in" className="download-button mt-6">Continue with email</a>
          </>
        ) : (
          <>
            <h2 className="font-medium text-ink">{status === 403 ? "Account unavailable" : "Accounts aren’t available right now"}</h2>
            <p role="status" className="mt-3 text-sm leading-relaxed text-ink-muted">
              {status === 403 ? "This account cannot access cloud features." : "Please try again later. You can still use Captures without signing in."}
            </p>
            <a href="/account" className="inline-chip mt-6 px-3 py-2">Try again</a>
            {status === 403 && <form action="/api/auth/sign-out" method="post" className="mt-4"><button type="submit" className="inline-chip px-3 py-2">Sign out</button></form>}
          </>
        )}
      </section>
    </main>
  );
}
