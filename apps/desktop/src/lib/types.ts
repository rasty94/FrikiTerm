// Copia a mano de los DTO de src-tauri/src/dto.rs. Solo T0 lo toca; un cambio de contrato se pide al coordinador.
// Los tipos de friki-core (SessionConfig, Forward) conservan su serde nativo (campos snake_case, enums kebab-case).

export type TermId = number;
export type ConnId = number;
export type SftpId = number;
export type PromptId = number;
export type JobId = number;
export type ForwardId = number;

export type ErrorKind =
  | "auth"
  | "hostkey"
  | "network"
  | "notfound"
  | "denied"
  | "invalid"
  | "unsupported"
  | "other";

export interface CmdError {
  kind: ErrorKind;
  message: string;
}

// ---- friki-core (serde nativo) ----

export type AuthMethod = "agent" | "public-key" | "keyboard-interactive" | "password";
export type Protocol = "ssh" | "telnet" | "telnet-tls" | "raw" | "tn3270" | "serial" | "local-shell";
export type Parity = "none" | "odd" | "even";
export type FlowControl = "none" | "software" | "hardware";
export type Emulation =
  | "xterm" | "vt100" | "vt220" | "linux" | "ansi" | "wyse50" | "wyse60" | "tvi910" | "tvi925";
export type BackspaceMode = "delete" | "ctrl-h";

export interface LogConfig {
  path: string;
  append: boolean;
  log_input: boolean;
}

export interface SerialConfig {
  device: string;
  baud: number;
  data_bits: number;
  parity: Parity;
  stop_bits: number;
  flow: FlowControl;
}

export interface TerminalProfile {
  emulation: Emulation;
  scrollback: number;
  backspace: BackspaceMode;
  theme: string;
  keymap: Record<string, string>;
}

export type Forward =
  | { kind: "local"; bind: string; port: number; host: string; host_port: number }
  | { kind: "remote"; bind: string; port: number; host: string; host_port: number }
  | { kind: "dynamic"; bind: string; port: number };

export interface SessionConfig {
  name: string;
  host: string;
  port: number;
  user: string | null;
  auth: AuthMethod[];
  identity_files: string[];
  proxy_jump: string | null;
  legacy_algorithms: boolean;
  term: string;
  max_auth_attempts: number;
  connect_timeout_secs: number;
  keepalive_secs: number | null;
  log: LogConfig | null;
  terminal: TerminalProfile;
  folder: string;
  protocol: Protocol;
  serial: SerialConfig;
  local_command: string | null;
  forwards: Forward[];
  forward_agent: boolean;
  require_strict_kex: boolean;
  pkcs11_provider: string | null;
  gssapi: boolean;
  gssapi_delegate: boolean;
}

// ---- Prompts y eventos globales (camelCase) ----

export interface KbdPrompt {
  text: string;
  echo: boolean;
}

export type Prompt =
  | { kind: "hostKeyUnknown"; host: string; port: number; algorithm: string; fingerprint: string }
  | {
      kind: "hostKeyChanged";
      host: string;
      port: number;
      algorithm: string;
      fingerprint: string;
      previousFingerprint: string;
    }
  | { kind: "password"; user: string; host: string; port: number }
  | { kind: "passphrase"; keyPath: string }
  | { kind: "kbdInteractive"; host: string; name: string; instructions: string; prompts: KbdPrompt[] };

export type HostKeyDecision = "reject" | "acceptOnce" | "acceptAndSave";

export type PromptAnswer =
  | { kind: "hostKey"; decision: HostKeyDecision }
  | { kind: "secret"; value: string | null; remember: boolean }
  | { kind: "multi"; values: string[] | null };

export type TransferEvent =
  | { type: "started"; id: JobId; file: string; total: number | null }
  | { type: "progress"; id: JobId; file: string; done: number }
  | { type: "retrying"; id: JobId; attempt: number; error: string }
  | { type: "finished"; id: JobId; bytes: number }
  | { type: "failed"; id: JobId; error: string; errorKind: ErrorKind }
  | { type: "cancelled"; id: JobId };

export interface ForwardStatus {
  id: ForwardId;
  spec: Forward;
  listen: string;
  active: boolean;
  error: string | null;
}

export type AppEvent =
  | { type: "prompt"; id: PromptId; prompt: Prompt }
  | { type: "transfer"; sftp: SftpId; ev: TransferEvent }
  | { type: "forward"; conn: ConnId; status: ForwardStatus }
  | { type: "connClosed"; conn: ConnId; reason: string | null }
  | { type: "promptDone"; id: PromptId };

// ---- Terminal ----

export type TermTarget =
  | { kind: "saved"; name: string }
  | { kind: "quick"; spec: SessionConfig }
  | { kind: "local" };

export type TermEvent =
  | {
      type: "opened";
      conn: ConnId | null;
      warnings: string[];
      negotiated: string | null;
      forwards: ForwardStatus[];
    }
  | { type: "failed"; error: CmdError }
  | { type: "exit"; code: number | null }
  | { type: "closed" }
  | { type: "lost"; reason: string | null };

// ---- Hosts ----

export interface ImportReport {
  added: number;
  warnings: string[];
}

// ---- SFTP y transferencias ----

export type FsRef = { kind: "local"; root: string } | { kind: "remote"; sftp: SftpId };

export type EntryKind = "file" | "dir" | "symlink" | "other";

export interface EntryDto {
  name: string;
  path: string;
  kind: EntryKind;
  size: number;
  mtime: number | null;
  mode: number | null;
}

export type Direction = "upload" | "download";

export interface XferJob {
  direction: Direction;
  src: string;
  dst: string;
  recursive: boolean;
}

// ---- Llavero y snippets ----

export interface KeyInfo {
  path: string;
  algorithm: string;
  comment: string;
  fingerprint: string;
  encrypted: boolean;
}

export interface Snippet {
  name: string;
  command: string;
}
