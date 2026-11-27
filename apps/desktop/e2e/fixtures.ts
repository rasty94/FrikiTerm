import { test as base, expect, type Page } from "@playwright/test";
import { installBackend } from "./mock-backend";

export type Call = { cmd: string; args: any };
export const calls = (page: Page, cmd: string): Promise<Call[]> =>
  page.evaluate((c) => (window as any).__mock.called(c), cmd);

// Toda prueba falla ante console.error, excepciones no capturadas o comandos IPC sin mock.
export const test = base.extend<{ errors: string[] }>({
  errors: [
    async ({ page }, use) => {
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
      page.on("console", (m) => m.type() === "error" && errors.push(`console.error: ${m.text()}`));
      await page.addInitScript(installBackend);
      await use(errors);
      const unhandled = await page.evaluate(() => (window as any).__mock?.unhandled ?? []).catch(() => []);
      expect(unhandled, "comandos IPC sin mock").toEqual([]);
      expect(errors, "errores de consola/página").toEqual([]);
    },
    { auto: true },
  ],
});
export { expect };

export async function shot(page: Page, name: string, scheme: string) {
  await page.waitForTimeout(300); // deja terminar las transiciones de estado (hover, vista activa)
  await page.screenshot({ path: `e2e/screenshots/${scheme}-${name}.png`, fullPage: true });
}
