import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

async function server(t, enabled) {
  const socket = createServer();
  socket.listen(0, "127.0.0.1");
  await once(socket, "listening");
  const port = socket.address().port;
  await new Promise((resolve) => socket.close(resolve));
  const base = `http://127.0.0.1:${port}`;
  const env = { ...process.env, HOST: "127.0.0.1", PORT: String(port) };
  for (const key of Object.keys(env)) {
    if (key.startsWith("WORKOS_") || key === "CAPTURES_API_URL") delete env[key];
  }
  if (enabled) Object.assign(env, {
    WORKOS_API_KEY: "test-only",
    WORKOS_CLIENT_ID: "client_test",
    WORKOS_COOKIE_PASSWORD: "test-only-cookie-password-at-least-32-characters",
    WORKOS_REDIRECT_URI: `${base}/api/auth/callback`,
    CAPTURES_API_URL: "http://captures-api.invalid",
  });
  const child = spawn(process.execPath, ["--import", "./tests/mock-account-services.mjs", ".output/server/index.mjs"], { env, stdio: "pipe" });
  let output = "";
  child.stdout.on("data", (data) => { output += data; });
  child.stderr.on("data", (data) => { output += data; });
  t.after(async () => {
    if (child.exitCode === null) {
      const exited = once(child, "exit");
      child.kill("SIGTERM");
      await exited;
    }
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    if (child.exitCode !== null) throw new Error(output);
    try {
      if ((await fetch(`${base}/api/health`)).ok) return base;
    } catch { /* Wait for the test server. */ }
    await sleep(50);
  }
  throw new Error(`Test server did not start: ${output}`);
}

function cookies(response) {
  return response.headers.getSetCookie().map((cookie) => cookie.split(";", 1)[0]).join("; ");
}

async function login(base, code = "test-code") {
  const start = await fetch(`${base}/api/auth/sign-in`, { redirect: "manual" });
  assert.equal(start.status, 302);
  assert.equal(start.headers.get("Cache-Control"), "no-store");
  assert.match(start.headers.get("Set-Cookie"), /HttpOnly/i);
  const workos = new URL(start.headers.get("Location"));
  assert.equal(workos.searchParams.get("client_id"), "client_test");
  assert.equal(workos.searchParams.get("code_challenge_method"), "S256");
  const callback = new URL("/api/auth/callback", base);
  callback.searchParams.set("code", code);
  callback.searchParams.set("state", workos.searchParams.get("state"));
  return fetch(callback, { headers: { Cookie: cookies(start) }, redirect: "manual" });
}

test("unconfigured accounts fail closed without breaking the public website", async (t) => {
  const base = await server(t, false);
  assert.equal((await fetch(base)).status, 200);
  assert.equal((await fetch(`${base}/api/account/me`)).status, 503);
  assert.equal((await fetch(`${base}/api/auth/sign-in`)).status, 503);
  const page = await fetch(`${base}/account`);
  assert.match(page.headers.get("Cache-Control"), /no-store/);
  assert.match(await page.text(), /Accounts aren’t available right now/);
});

test("AuthKit PKCE callback, account projection and CSRF-protected logout", async (t) => {
  const base = await server(t, true);
  const anonymous = await fetch(`${base}/api/account/me`, { headers: { Authorization: "Bearer browser-supplied-token" } });
  assert.equal(anonymous.status, 401);
  const signedOut = await fetch(`${base}/account`);
  assert.match(await signedOut.text(), /Continue with email/);

  const rejected = await fetch(`${base}/api/auth/callback?code=x&state=invalid`, { redirect: "manual" });
  assert.match(rejected.headers.get("Location"), /account\?error=sign-in/);
  assert.ok(!rejected.headers.getSetCookie().some((value) => value.startsWith("wos-session=")));

  const result = await login(base);
  assert.equal(result.status, 307);
  assert.match(result.headers.get("Location"), /\/account$/);
  const cookie = cookies(result);
  assert.match(cookie, /wos-session=/);
  const account = await fetch(`${base}/api/account/me`, { headers: { Cookie: cookie } });
  assert.equal(account.status, 200);
  assert.equal(account.headers.get("Cache-Control"), "no-store");
  assert.deepEqual(await account.json(), { email: "tester@example.com", emailVerified: true });
  const page = await fetch(`${base}/account`, { headers: { Cookie: cookie } });
  const html = await page.text();
  assert.match(html, /You’re signed in/);
  assert.ok(!html.includes("test-refresh-not-a-real-token"));
  assert.ok(!html.includes("user_test"));

  const crossSite = await fetch(`${base}/api/auth/sign-out`, { method: "POST", headers: { Cookie: cookie, Origin: "https://attacker.invalid", "Sec-Fetch-Site": "cross-site" }, redirect: "manual" });
  assert.equal(crossSite.status, 403);
  const logout = await fetch(`${base}/api/auth/sign-out`, { method: "POST", headers: { Cookie: cookie, Origin: base, "Sec-Fetch-Site": "same-origin" }, redirect: "manual" });
  assert.equal(logout.status, 303);
  assert.match(logout.headers.get("Location"), /user_management\/sessions\/logout/);
  assert.match(logout.headers.get("Set-Cookie"), /wos-session=;/);
  const blocked = await login(base, "blocked");
  assert.equal(blocked.status, 307);
  // WorkOS authentication is not local authorization: Rust still denies access.
  const blockedHeaders = { Cookie: cookies(blocked) };
  assert.equal((await fetch(`${base}/api/account/me`, { headers: blockedHeaders })).status, 403);
  const blockedPage = await fetch(`${base}/account`, { headers: blockedHeaders });
  assert.match(await blockedPage.text(), /This account cannot access cloud features/);
});
