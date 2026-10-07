import { loadConfig } from "./config";

const STATE_KEY = "discord-oauth-state";

/** Where Discord sends members back after its consent screen. */
export function discordRedirectUri(): string {
  return `${window.location.origin}/connect/discord/callback/`;
}

/** Sends the member to Discord's consent screen with a fresh anti-forgery state. */
export async function startDiscordConnect(scopes: string): Promise<void> {
  const { discordClientId } = await loadConfig();
  if (!discordClientId) {
    throw new Error("Discord linking isn't available here.");
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  const state = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  sessionStorage.setItem(STATE_KEY, state);
  const url = new URL("https://discord.com/oauth2/authorize");
  url.searchParams.set("client_id", discordClientId);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("redirect_uri", discordRedirectUri());
  url.searchParams.set("scope", scopes);
  url.searchParams.set("state", state);
  window.location.assign(url);
}

/** Whether `state` is the one this tab started with. Each state works once. */
export function consumeDiscordState(state: string | null): boolean {
  const expected = sessionStorage.getItem(STATE_KEY);
  sessionStorage.removeItem(STATE_KEY);
  return expected !== null && state === expected;
}
