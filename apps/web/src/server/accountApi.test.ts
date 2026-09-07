import assert from "node:assert/strict";
import test from "node:test";
import { fetchAccount } from "./accountApi.ts";
import { accountsConfigured } from "./accountConfig.ts";

test("accounts are opt-in and require complete runtime configuration", () => {
  const env = {
    WORKOS_CLIENT_ID: "client_test",
    WORKOS_API_KEY: "test-key",
    WORKOS_REDIRECT_URI: "https://example.com/api/auth/callback",
    WORKOS_COOKIE_PASSWORD: "x".repeat(32),
    CAPTURES_API_URL: "http://api:3001",
  };
  assert.equal(accountsConfigured({}), false);
  assert.equal(accountsConfigured(env), true);
  for (const key of Object.keys(env)) {
    assert.equal(accountsConfigured({ ...env, [key]: "" }), false, key);
  }
  assert.equal(accountsConfigured({ ...env, WORKOS_COOKIE_PASSWORD: "short" }), false);
});

test("only the session token is forwarded and only account fields are returned", async () => {
  const response = await fetchAccount("server-token", "http://api:3001", async (url, init) => {
    assert.equal(String(url), "http://api:3001/api/account/me");
    assert.deepEqual(init?.headers, { Authorization: "Bearer server-token" });
    assert.equal(init?.redirect, "error");
    assert.equal(init?.cache, "no-store");
    return Response.json({ email: "person@example.com", emailVerified: true, id: 123, workos_user_id: "private", accessToken: "private" });
  });
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("Cache-Control"), "no-store");
  assert.deepEqual(await response.json(), { email: "person@example.com", emailVerified: true });
});

test("upstream failures fail closed without leaking bodies", async () => {
  for (const status of [401, 403, 404, 429, 500, 503]) {
    const response = await fetchAccount("token", "http://api:3001", async () => new Response("sensitive error", { status }));
    assert.equal(response.status, [401, 403].includes(status) ? status : 503);
    assert.equal(response.headers.get("Cache-Control"), "no-store");
    assert.ok(!(await response.text()).includes("sensitive"));
  }
});

test("network failures and malformed account responses are unavailable", async () => {
  const failure = await fetchAccount("token", "http://api:3001", async () => { throw new Error("secret"); });
  assert.equal(failure.status, 503);
  for (const data of [null, {}, { email: "e" }, { email: 1, emailVerified: true }]) {
    const response = await fetchAccount("token", "http://api:3001", async () => Response.json(data));
    assert.equal(response.status, 503);
  }
});
