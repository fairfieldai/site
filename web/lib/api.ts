import { getUser } from "./auth";

/** Calls the site API as the signed-in member. */
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const user = await getUser();
  if (!user) {
    throw new Error("Not signed in");
  }
  const headers = new Headers(init.headers);
  headers.set("Authorization", `Bearer ${user.access_token}`);
  if (init.body) {
    headers.set("Content-Type", "application/json");
  }
  return fetch(path, { ...init, headers });
}

/** The error message from an API response, or a fallback. */
export async function apiError(response: Response, fallback: string): Promise<string> {
  try {
    const body = (await response.json()) as { error?: string };
    return body.error ?? fallback;
  } catch {
    return fallback;
  }
}
