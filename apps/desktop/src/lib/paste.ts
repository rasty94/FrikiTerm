/** Un pegado con saltos de línea (salvo uno final) ejecutaría comandos sin que el usuario los vea: se pide confirmación. */
export function needsPasteConfirm(text: string): boolean {
  return /[\r\n]/.test(text.replace(/\r?\n$|\r$/, ""));
}
