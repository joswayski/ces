import { getAuthKitContext } from "@workos/authkit-tanstack-react-start";
import { accountError, accountsConfigured } from "./accountConfig.ts";
import { fetchAccount } from "./accountApi.ts";

export async function currentAccount(): Promise<Response> {
  if (!accountsConfigured()) return accountError(503);
  const auth = getAuthKitContext().auth();
  if (!auth.user || !auth.accessToken) return accountError(401);
  return fetchAccount(auth.accessToken, process.env.CAPTURES_API_URL!);
}
