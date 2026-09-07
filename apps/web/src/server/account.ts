export function currentAccount(): Response {
  return Response.json(
    { error: "Accounts are not available" },
    { status: 503, headers: { "Cache-Control": "no-store" } },
  );
}
