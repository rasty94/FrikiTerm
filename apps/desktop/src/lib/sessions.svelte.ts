// Estado compartido de sesiones SSH (conexiones, pestañas de terminal, transferencias y redirecciones).
// Lo alimentan las vistas T5 y App.svelte (T4), que debe pasar los AppEvent no-prompt a handleAppEvent.
import type { AppEvent, ConnId, ForwardStatus, JobId, SftpId, TermId } from "./types";

export interface ConnInfo {
  id: ConnId;
  host: string;
  sftp: SftpId | null;
}

export interface TermTab {
  id: TermId;
  label: string;
}

export type JobStatus = "queued" | "running" | "retrying" | "done" | "failed" | "cancelled";

export interface Job {
  sftp: SftpId;
  id: JobId;
  file: string;
  done: number;
  total: number | null;
  status: JobStatus;
  error: string | null;
}

export const sessions = $state({
  conns: [] as ConnInfo[],
  terms: [] as TermTab[],
  jobs: [] as Job[],
  forwards: {} as Record<number, ForwardStatus[]>,
});

export function addConn(id: ConnId, host: string): void {
  if (!sessions.conns.some((c) => c.id === id)) sessions.conns.push({ id, host, sftp: null });
}

export function setConnSftp(conn: ConnId, sftp: SftpId): void {
  const c = sessions.conns.find((x) => x.id === conn);
  if (c) c.sftp = sftp;
}

export function removeConn(conn: ConnId): void {
  sessions.conns = sessions.conns.filter((c) => c.id !== conn);
  delete sessions.forwards[conn];
}

export function addTerm(tab: TermTab): void {
  if (!sessions.terms.some((t) => t.id === tab.id)) sessions.terms.push(tab);
}

export function removeTerm(id: TermId): void {
  sessions.terms = sessions.terms.filter((t) => t.id !== id);
}

/** Registra jobs recién encolados como "queued" para que la cola los muestre al instante. */
export function queueJobs(sftp: SftpId, ids: JobId[], files: string[]): void {
  ids.forEach((id, i) => {
    sessions.jobs.push({ sftp, id, file: files[i] ?? "", done: 0, total: null, status: "queued", error: null });
  });
}

export function clearFinishedJobs(): void {
  sessions.jobs = sessions.jobs.filter((j) => j.status === "queued" || j.status === "running" || j.status === "retrying");
}

function upsertJob(sftp: SftpId, id: JobId): Job {
  let job = sessions.jobs.find((j) => j.sftp === sftp && j.id === id);
  if (!job) {
    job = { sftp, id, file: "", done: 0, total: null, status: "queued", error: null };
    sessions.jobs.push(job);
  }
  return job;
}

export function handleAppEvent(e: AppEvent): void {
  if (e.type === "transfer") {
    const j = upsertJob(e.sftp, e.ev.id);
    const ev = e.ev;
    switch (ev.type) {
      case "started":
        j.status = "running";
        j.file = ev.file;
        j.total = ev.total;
        break;
      case "progress":
        j.status = "running";
        j.file = ev.file;
        j.done = ev.done;
        break;
      case "retrying":
        j.status = "retrying";
        j.error = ev.error;
        break;
      case "finished":
        j.status = "done";
        j.done = ev.bytes;
        j.total = ev.bytes;
        break;
      case "failed":
        j.status = "failed";
        j.error = ev.error;
        break;
      case "cancelled":
        j.status = "cancelled";
        break;
    }
  } else if (e.type === "forward") {
    const list = sessions.forwards[e.conn] ?? [];
    const i = list.findIndex((f) => f.id === e.status.id);
    if (i >= 0) list[i] = e.status;
    else list.push(e.status);
    sessions.forwards[e.conn] = list;
  } else if (e.type === "connClosed") {
    removeConn(e.conn);
  }
}
