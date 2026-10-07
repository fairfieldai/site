// Per-environment settings. The deploy writes /config.json from the GitHub
// Environment's variables, so one build serves dev and prod. For `pnpm dev`,
// create public/config.json (see README).
export interface SiteConfig {
  cognitoDomain: string;
  cognitoClientId: string;
  cognitoIssuer: string;
}

let config: Promise<SiteConfig> | undefined;

export function loadConfig(): Promise<SiteConfig> {
  config ??= fetch("/config.json", { cache: "no-store" }).then((response) => {
    if (!response.ok) {
      throw new Error(`Failed to load /config.json: HTTP ${response.status}`);
    }
    return response.json() as Promise<SiteConfig>;
  });
  return config;
}
