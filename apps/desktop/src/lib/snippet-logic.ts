// Lógica pura de snippets: validación de la lista y bytes que se envían a term_write.
import type { Snippet } from "./types";

/** Devuelve el primer problema de la lista, o null si es válida. */
export function validateSnippets(list: Snippet[]): string | null {
  const seen = new Set<string>();
  for (const s of list) {
    const name = s.name.trim();
    if (name === "") return "Todos los snippets necesitan nombre";
    if (seen.has(name)) return `Nombre duplicado: ${name}`;
    seen.add(name);
    if (s.command.trim() === "") return `El snippet «${name}» está vacío`;
  }
  return null;
}

/** Bytes para term_write. Con `newline`, se añade el "\r" que espera un terminal al pulsar Enter. */
export function snippetBytes(command: string, newline: boolean): Uint8Array {
  return new TextEncoder().encode(newline ? `${command}\r` : command);
}
