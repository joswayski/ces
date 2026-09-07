import { createFileRoute } from "@tanstack/react-router";
import { getSignInUrl } from "@workos/authkit-tanstack-react-start";
import { accountError, accountsConfigured } from "../../../server/accountConfig";

export const Route = createFileRoute("/api/auth/sign-in")({
  server: {
    handlers: {
      GET: async () => {
        if (!accountsConfigured()) return accountError(503);
        const url = await getSignInUrl({ data: { returnPathname: "/account" } });
        return new Response(null, {
          status: 302,
          headers: { Location: url, "Cache-Control": "no-store" },
        });
      },
    },
  },
});
