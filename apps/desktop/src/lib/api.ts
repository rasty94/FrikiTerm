// Único sitio que llama a `invoke`. Un envoltorio tipado por comando de Rust.
import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AppEvent, ConnId, DesktopSettings, EntryDto, Forward, ForwardId, ForwardStatus, FsRef, ImportReport, JobId,
  KeyInfo, PromptAnswer, PromptId, SessionConfig, SftpId, Snippet, TermEvent, TermId, TermTarget,
  XferJob,
} from "./types";

// ---- Eventos globales ----

export function appSubscribe(onEvent: (e: AppEvent) => void): Promise<void> {
  const events = new Channel<AppEvent>();
  events.onmessage = onEvent;
  return invoke("app_subscribe", { events });
}

export const promptAnswer = (id: PromptId, answer: PromptAnswer): Promise<void> =>
  invoke("prompt_answer", { id, answer });

// ---- Hosts ----

export const hostsList = (): Promise<SessionConfig[]> => invoke("hosts_list");
export const hostSave = (host: SessionConfig, previousName: string | null): Promise<void> =>
  invoke("host_save", { host, previousName });
export const hostDelete = (name: string): Promise<void> => invoke("host_delete", { name });
export const hostsImportSshConfig = (): Promise<ImportReport> => invoke("hosts_import_ssh_config");

// ---- Terminal ----

/** `onOutput` recibe bytes crudos (ArrayBuffer); `onEvent` los TermEvent. Vuelve al momento. */
export function termOpen(
  target: TermTarget,
  cols: number,
  rows: number,
  onOutput: (data: ArrayBuffer) => void,
  onEvent: (e: TermEvent) => void,
): Promise<TermId> {
  const output = new Channel<ArrayBuffer>();
  output.onmessage = onOutput;
  const events = new Channel<TermEvent>();
  events.onmessage = onEvent;
  return invoke("term_open", { target, cols, rows, output, events });
}

// Vec<u8> en Rust: se manda como array JSON (solo teclado/snippets, trozos pequeños).
export const termWrite = (id: TermId, data: Uint8Array): Promise<void> =>
  invoke("term_write", { id, data: Array.from(data) });
export const termResize = (id: TermId, cols: number, rows: number): Promise<void> =>
  invoke("term_resize", { id, cols, rows });
export const termClose = (id: TermId): Promise<void> => invoke("term_close", { id });

// ---- Conexiones (solo SSH) ----

export const connOpen = (host: string): Promise<ConnId> => invoke("conn_open", { host });
export const connClose = (conn: ConnId): Promise<void> => invoke("conn_close", { conn });

// ---- SFTP y transferencias ----

export const sftpOpen = (conn: ConnId): Promise<SftpId> => invoke("sftp_open", { conn });
export const localRoot = (): Promise<string> => invoke("local_root");
export const localPickRoot = (): Promise<string | null> => invoke("local_pick_root");
export const fsList = (fs: FsRef, path: string): Promise<EntryDto[]> =>
  invoke("fs_list", { fs, path });
export const fsMkdir = (fs: FsRef, path: string): Promise<void> => invoke("fs_mkdir", { fs, path });
export const fsRename = (fs: FsRef, from: string, to: string): Promise<void> =>
  invoke("fs_rename", { fs, from, to });
export const fsRemove = (fs: FsRef, path: string, recursive: boolean): Promise<void> =>
  invoke("fs_remove", { fs, path, recursive });
export const xferEnqueue = (sftp: SftpId, localRoot: string, jobs: XferJob[]): Promise<JobId[]> =>
  invoke("xfer_enqueue", { sftp, localRoot, jobs });
export const xferUploadPaths = (sftp: SftpId, paths: string[], remoteDir: string): Promise<JobId[]> =>
  invoke("xfer_upload_paths", { sftp, paths, remoteDir });
export const xferCancel = (sftp: SftpId, job: JobId): Promise<void> =>
  invoke("xfer_cancel", { sftp, job });
export const xferPause = (sftp: SftpId, paused: boolean): Promise<void> =>
  invoke("xfer_pause", { sftp, paused });

// ---- Llavero y snippets ----

export const keysList = (): Promise<KeyInfo[]> => invoke("keys_list");
export const keyGenerate = (name: string, comment: string, passphrase: string | null): Promise<KeyInfo> =>
  invoke("key_generate", { name, comment, passphrase });
export const keyImport = (): Promise<KeyInfo | null> => invoke("key_import");
export const keyInstall = (conn: ConnId, keyPath: string): Promise<boolean> =>
  invoke("key_install", { conn, keyPath });
export const snippetsList = (): Promise<Snippet[]> => invoke("snippets_list");
export const snippetsSave = (snippets: Snippet[]): Promise<void> =>
  invoke("snippets_save", { snippets });

// ---- Redirecciones ----

export const fwdList = (conn: ConnId): Promise<ForwardStatus[]> => invoke("fwd_list", { conn });
export const fwdStart = (conn: ConnId, spec: Forward): Promise<ForwardStatus> =>
  invoke("fwd_start", { conn, spec });
export const fwdStop = (conn: ConnId, id: ForwardId): Promise<void> => invoke("fwd_stop", { conn, id });

// ---- Ajustes y utilidades ----

export const settingsGet = (): Promise<DesktopSettings> => invoke("settings_get");
export const settingsSave = (settings: DesktopSettings): Promise<void> =>
  invoke("settings_save", { settings });
/** Solo http(s); el backend lo abre en el navegador del SO. */
export const openUrl = (url: string): Promise<void> => invoke("open_url", { url });
