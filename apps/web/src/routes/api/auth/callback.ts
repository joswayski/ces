import { createFileRoute } from "@tanstack/react-router";
import { handleCallbackRoute } from "@workos/authkit-tanstack-react-start";
import { accountError, accountsConfigured } from "../../../server/accountConfig";

// Authentication is separate from local authorization. The /account loader
// calls Rust to provision/resolve the account and enforce disabled/deleted state.
const callback = handleCallbackRoute({
  returnPathname: "/account",
  errorRedirectUrl: "/account?error=sign-in",
});

export const Route = createFileRoute("/api/auth/callback")({
  server: {
    handlers: {
      GET: async (ctx) => {
        if (!accountsConfigured()) return accountError(503);
        const response = await callback(ctx);
        response.headers.set("Cache-Control", "no-store");
        return response;
      },
    },
  },
});
