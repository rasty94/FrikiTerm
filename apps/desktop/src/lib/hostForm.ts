// Validación pura del formulario de hosts (sin DOM ni IPC). El backend valida lo mismo en `hosts::validate`.
import type { Forward, SessionConfig } from "./types";

/** Velocidades habituales; el formulario acepta además cualquier entero positivo. */
export const BAUD_RATES = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600] as const;
export const DATA_BITS = [5, 6, 7, 8] as const;
export const STOP_BITS = [1, 2] as const;

export type HostField =
  | "name"
  | "host"
  | "port"
  | "device"
  | "baud"
  | "dataBits"
  | "stopBits"
  | "timeout"
  | "keepalive"
  | "maxAuth"
  | "forwards";

/** Mensaje por campo; ausente = válido. */
export type HostErrors = Partial<Record<HostField, string>>;

export const isPort = (n: unknown): boolean => Number.isInteger(n) && (n as number) >= 1 && (n as number) <= 65535;

const intAtLeast = (n: unknown, min: number): boolean => Number.isInteger(n) && (n as number) >= min;

/** Velocidad personalizada: entero positivo. */
export function validateBaud(baud: unknown): string | null {
  return intAtLeast(baud, 1) ? null : "Velocidad: entero positivo (p. ej. 115200)";
}

/** Redirección: puertos válidos y, salvo dinámica, destino. Devuelve el primer problema o null. */
export function validateForward(f: Forward): string | null {
  if (!f.bind.trim()) return "dirección de escucha vacía";
  if (!isPort(f.port)) return "puerto de escucha entre 1 y 65535";
  if (f.kind === "dynamic") return null;
  if (!f.host.trim()) return "falta el host destino";
  if (!isPort(f.host_port)) return "puerto destino entre 1 y 65535";
  return null;
}

/** Errores de un borrador. Solo mira los campos que se editan para el protocolo elegido. */
export function validateSession(s: SessionConfig): HostErrors {
  const e: HostErrors = {};
  if (!s.name.trim()) e.name = "Falta el nombre";

  if (s.protocol === "local-shell") return e;

  if (s.protocol === "serial") {
    if (!s.serial.device.trim()) e.device = "Elige o escribe un dispositivo";
    const baud = validateBaud(s.serial.baud);
    if (baud) e.baud = baud;
    if (!(DATA_BITS as readonly number[]).includes(s.serial.data_bits)) e.dataBits = "Entre 5 y 8";
    if (!(STOP_BITS as readonly number[]).includes(s.serial.stop_bits)) e.stopBits = "1 o 2";
    return e;
  }

  if (!s.host.trim()) e.host = "Falta el host";
  if (!isPort(s.port)) e.port = "Puerto entre 1 y 65535";

  if (s.protocol === "ssh") {
    if (!intAtLeast(s.connect_timeout_secs, 1)) e.timeout = "Entero de 1 o más";
    if (s.keepalive_secs !== null && !intAtLeast(s.keepalive_secs, 0)) e.keepalive = "Entero de 0 o más (0 desactiva)";
    if (!intAtLeast(s.max_auth_attempts, 1)) e.maxAuth = "Entero de 1 o más";
    const bad = s.forwards.map((f, i) => (validateForward(f) ? i + 1 : 0)).find((i) => i > 0);
    if (bad) e.forwards = `Redirección ${bad}: ${validateForward(s.forwards[bad - 1])}`;
  }
  return e;
}

export const hasErrors = (e: HostErrors): boolean => Object.values(e).some(Boolean);

/** Líneas de texto a rutas: sin vacías ni espacios sobrantes. */
export const parseIdentityText = (text: string): string[] =>
  text.split("\n").map((s) => s.trim()).filter(Boolean);
