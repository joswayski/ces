import { createCsrfMiddleware, createStart } from "@tanstack/react-start";
import { authkitMiddleware } from "@workos/authkit-tanstack-react-start";
import { accountsConfigured } from "./server/accountConfig";

export const startInstance = createStart(() => ({
  requestMiddleware: [
    // A custom start instance replaces Start's default CSRF middleware.
    createCsrfMiddleware({
      filter: (ctx) => ctx.handlerType === "serverFn" ||
        new URL(ctx.request.url).pathname === "/api/auth/sign-out",
    }),
    // The public website still runs when accounts have not been configured.
    ...(accountsConfigured() ? [authkitMiddleware()] : []),
  ],
}));
