import { createFileRoute, isRedirect } from "@tanstack/react-router";
import { signOut } from "@workos/authkit-tanstack-react-start";
import { accountError, accountsConfigured } from "../../../server/accountConfig";

export const Route = createFileRoute("/api/auth/sign-out")({
  server: {
    handlers: {
      POST: async () => {
        if (!accountsConfigured()) return accountError(503);
        const returnTo = new URL("/account", process.env.WORKOS_REDIRECT_URI).href;
        try {
          await signOut({ data: { returnTo } });
        } catch (error) {
          if (!isRedirect(error)) throw error;
          const headers = new Headers(error.headers);
          headers.set("Location", error.options.href ?? returnTo);
          headers.set("Cache-Control", "no-store");
          // A form POST must become a GET at WorkOS's logout endpoint.
          return new Response(null, { status: 303, headers });
        }
      },
    },
  },
});
