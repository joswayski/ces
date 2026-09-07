// Loaded ONLY by the integration-test subprocess, never by application code.
// No real WorkOS requests, credentials, email deliveries, or database writes.
import { generateKeyPairSync, sign } from "node:crypto";

const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: "jwk" }), kid: "test", alg: "RS256", use: "sig" };
const encoded = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
function token(subject) {
  const now = Math.floor(Date.now() / 1000);
  const unsigned = `${encoded({ alg: "RS256", kid: "test" })}.${encoded({ sub: subject, sid: "session_test", iss: "https://api.workos.com/", iat: now, exp: now + 600 })}`;
  return `${unsigned}.${sign("RSA-SHA256", Buffer.from(unsigned), privateKey).toString("base64url")}`;
}

globalThis.fetch = async (input, init) => {
  const url = new URL(input instanceof Request ? input.url : input);
  if (url.hostname === "api.workos.com" && url.pathname.startsWith("/sso/jwks/")) {
    return Response.json({ keys: [jwk] });
  }
  if (url.hostname === "api.workos.com" && url.pathname === "/user_management/authenticate") {
    const body = JSON.parse(init.body);
    const subject = body.code === "blocked" ? "user_blocked" : "user_test";
    return Response.json({
      access_token: token(subject), refresh_token: "test-refresh-not-a-real-token",
      authentication_method: "MagicAuth",
      user: { object: "user", id: subject, email: "tester@example.com", email_verified: true, first_name: null, last_name: null, profile_picture_url: null, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
    });
  }
  if (url.hostname === "captures-api.invalid" && url.pathname === "/api/account/me") {
    const jwt = new Headers(init?.headers).get("Authorization")?.slice(7);
    const { sub } = JSON.parse(Buffer.from(jwt.split(".")[1], "base64url"));
    return sub === "user_blocked"
      ? Response.json({ error: "Forbidden" }, { status: 403 })
      : Response.json({ email: "tester@example.com", emailVerified: true });
  }
  throw new Error(`Unexpected external request in account test: ${url.origin}${url.pathname}`);
};
