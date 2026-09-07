import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

async function server(t) {
  const socket = createServer();
  socket.listen(0, "127.0.0.1");
  await once(socket, "listening");
  const port = socket.address().port;
  await new Promise((resolve) => socket.close(resolve));
  const base = `http://127.0.0.1:${port}`;
  const env = { ...process.env, HOST: "127.0.0.1", PORT: String(port) };
  const child = spawn(process.execPath, [".output/server/index.mjs"], { env, stdio: "pipe" });
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

test("accounts stay unavailable without breaking the public website", async (t) => {
  const base = await server(t);
  assert.equal((await fetch(base)).status, 200);
  for (const headers of [{}, { Authorization: "Bearer obsolete-token", Cookie: "session=obsolete" }]) {
    const response = await fetch(`${base}/api/account/me`, { headers });
    assert.equal(response.status, 503);
    assert.equal(response.headers.get("Cache-Control"), "no-store");
    assert.equal(response.headers.get("Set-Cookie"), null);
    assert.deepEqual(await response.json(), { error: "Accounts are not available" });
  }
  for (const [path, method] of [["sign-in", "GET"], ["callback?code=obsolete&state=obsolete", "GET"], ["sign-out", "POST"]]) {
    const response = await fetch(`${base}/api/auth/${path}`, { method, redirect: "manual" });
    assert.equal(response.status, 404);
    assert.equal(response.headers.get("Location"), null);
    assert.equal(response.headers.get("Set-Cookie"), null);
  }
  for (const path of ["/account", "/account?error=sign-in"]) {
    const page = await fetch(`${base}${path}`);
    assert.equal(page.status, 200);
    assert.match(page.headers.get("Cache-Control"), /no-store/);
    const html = await page.text();
    assert.match(html, /Accounts aren’t available right now/);
    assert.doesNotMatch(html, /Continue with email|Sign out|one-time code/);
  }
});
