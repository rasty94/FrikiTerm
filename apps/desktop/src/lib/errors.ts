// Texto legible para CmdError (o cualquier cosa que lance invoke).
import type { CmdError } from "./types";

export function errorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) {
    const err = e as CmdError;
    if (err.kind === "unsupported") return "Función no disponible todavía";
    return err.message;
  }
  return String(e);
}
