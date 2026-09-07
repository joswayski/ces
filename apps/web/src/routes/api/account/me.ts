import { createFileRoute } from "@tanstack/react-router";
import { currentAccount } from "../../../server/account";

export const Route = createFileRoute("/api/account/me")({
  server: { handlers: { GET: () => currentAccount() } },
});
