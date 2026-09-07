import { createCsrfMiddleware, createMiddleware, createStart } from "@tanstack/react-start";

const accountResponses = createMiddleware().server(async ({ request, next }) => {
  const result = await next();
  if (new URL(request.url).pathname === "/account") {
    result.response.headers.set("Cache-Control", "no-store");
  }
  return result;
});

export const startInstance = createStart(() => ({
  requestMiddleware: [
    accountResponses,
    // A custom start instance replaces Start's default CSRF middleware.
    createCsrfMiddleware({ filter: (ctx) => ctx.handlerType === "serverFn" }),
  ],
}));
