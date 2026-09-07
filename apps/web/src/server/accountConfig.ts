/** Runtime-only configuration. No database credentials belong in the web app. */
export function accountsConfigured(env: NodeJS.ProcessEnv = process.env): boolean {
  return Boolean(
    env.WORKOS_CLIENT_ID?.trim() &&
      env.WORKOS_API_KEY?.trim() &&
      env.WORKOS_REDIRECT_URI?.trim() &&
      env.WORKOS_COOKIE_PASSWORD &&
      env.WORKOS_COOKIE_PASSWORD.length >= 32 &&
      env.CAPTURES_API_URL?.trim(),
  );
}

export function accountError(status: number): Response {
  return Response.json(
    { error: status === 401 ? "Sign in required" : status === 403 ? "Account unavailable" : "Accounts temporarily unavailable" },
    { status, headers: { "Cache-Control": "no-store" } },
  );
}
