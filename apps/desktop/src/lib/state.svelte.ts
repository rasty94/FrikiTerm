// Estado global mínimo con runes.
import type { PromptId, Prompt, SessionConfig } from "./types";

export type ViewName = "hosts" | "sftp" | "keychain" | "snippets" | "forwards";

export interface PendingPrompt {
  id: PromptId;
  prompt: Prompt;
}

export const app = $state({
  view: "hosts" as ViewName,
  hosts: [] as SessionConfig[],
  /** Cola FIFO: el modal muestra el primero. */
  prompts: [] as PendingPrompt[],
});
