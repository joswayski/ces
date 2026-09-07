import { accountError } from "./accountConfig.ts";

export interface Account {
  email: string;
  emailVerified: boolean;
}

/** Only the server's WorkOS session token is forwarded, never browser headers. */
export async function fetchAccount(
  accessToken: string,
  baseUrl: string,
  fetcher: typeof fetch = fetch,
): Promise<Response> {
  try {
    const upstream = await fetcher(new URL("/api/account/me", baseUrl), {
      headers: { Authorization: `Bearer ${accessToken}` },
      redirect: "error",
      cache: "no-store",
      signal: AbortSignal.timeout(12_000),
    });
    if (!upstream.ok) {
      return accountError([401, 403].includes(upstream.status) ? upstream.status : 503);
    }
    const data: unknown = await upstream.json();
    if (
      !data || typeof data !== "object" ||
      !("email" in data) || typeof data.email !== "string" ||
      !("emailVerified" in data) || typeof data.emailVerified !== "boolean"
    ) {
      return accountError(503);
    }
    // Explicit projection: upstream IDs, tokens and future fields stay private.
    return Response.json(
      { email: data.email, emailVerified: data.emailVerified } satisfies Account,
      { headers: { "Cache-Control": "no-store" } },
    );
  } catch {
    return accountError(503);
  }
}
