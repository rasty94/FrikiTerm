import type { SessionConfig } from "./types";

/** SessionConfig con los mismos valores por defecto que friki-core. */
export function defaultSession(p: Partial<SessionConfig> & { host: string }): SessionConfig {
  return {
    name: p.host,
    port: 22,
    user: null,
    auth: ["agent", "public-key", "keyboard-interactive", "password"],
    identity_files: [],
    proxy_jump: null,
    legacy_algorithms: false,
    term: "xterm-256color",
    max_auth_attempts: 3,
    connect_timeout_secs: 15,
    keepalive_secs: 30,
    log: null,
    terminal: { emulation: "xterm", scrollback: 10000, backspace: "delete", theme: "", keymap: {} },
    folder: "",
    protocol: "ssh",
    serial: { device: "", baud: 115200, data_bits: 8, parity: "none", stop_bits: 1, flow: "none" },
    local_command: null,
    forwards: [],
    forward_agent: false,
    require_strict_kex: false,
    pkcs11_provider: null,
    gssapi: false,
    gssapi_delegate: false,
    ...p,
  };
}

/** `usuario@host:puerto`; usuario y puerto son opcionales. null si no encaja. */
export function parseQuickSpec(spec: string): { user: string | null; host: string; port: number } | null {
  const m = /^(?:([^@\s]+)@)?([^:@\s]+)(?::(\d{1,5}))?$/.exec(spec.trim());
  if (!m) return null;
  const port = m[3] ? Number(m[3]) : 22;
  if (port < 1 || port > 65535) return null;
  return { user: m[1] ?? null, host: m[2], port };
}

/** Los comandos Tauri rechazan con un CmdError `{ kind, message }`; cualquier otra cosa se convierte a texto. */
export function errMsg(e: unknown): string {
  if (e && typeof e === "object" && "message" in e && typeof e.message === "string") return e.message;
  return String(e);
}
