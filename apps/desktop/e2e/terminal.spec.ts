import { calls, expect, shot, test } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
});

const plus = (page: any) => page.getByRole("button", { name: "Nueva pestaña" });
const openWeb = async (page: any) => {
  await page.getByTitle("Conectar a web-01").click();
  await expect(page.getByText("Conectado").first()).toBeVisible();
};

test("menú «+»: terminal local, conexión rápida con historial y hosts con búsqueda", async ({ page }) => {
  await page.getByRole("navigation", { name: "Principal" }).getByRole("button", { name: "Terminal" }).click();
  await plus(page).click();
  const menu = page.getByRole("dialog", { name: "Nueva pestaña" });
  await expect(menu.getByRole("button", { name: "Terminal local" })).toBeVisible();
  await expect(menu.getByRole("list", { name: "Hosts guardados" }).getByRole("button")).toHaveCount(4);
  await menu.getByRole("searchbox", { name: "Buscar host guardado" }).fill("web");
  await expect(menu.getByRole("list", { name: "Hosts guardados" }).getByRole("button")).toHaveCount(2);

  await menu.getByRole("button", { name: "Terminal local" }).click();
  await expect(menu).toHaveCount(0);
  expect((await calls(page, "term_open"))[0].args.target).toEqual({ kind: "local" });

  await plus(page).click();
  await page.getByLabel("Conexión rápida (usuario@host:puerto)").fill("ana@box:2200");
  await page.keyboard.press("Enter");
  const t = (await calls(page, "term_open"))[1].args.target;
  expect(t.kind).toBe("quick");
  expect([t.spec.user, t.spec.host, t.spec.port]).toEqual(["ana", "box", 2200]);

  await plus(page).click();
  await expect(page.getByRole("button", { name: "ana@box:2200" })).toBeVisible();
  await page.getByRole("button", { name: "web-02", exact: false }).first().click();
  expect((await calls(page, "term_open"))[2].args.target).toEqual({ kind: "saved", name: "web-02" });
});

test("enviar a todos: refleja la entrada en los paneles abiertos y se puede apagar", async ({ page }, info) => {
  await openWeb(page);
  await page.getByRole("button", { name: "Dividir a la derecha" }).click();
  await expect(page.getByText("Conectado")).toHaveCount(2);
  const toggle = page.getByRole("button", { name: "Enviar a todos", exact: true });
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".badge-bc")).toHaveCount(2);

  const ids = async () => new Set((await calls(page, "term_write")).map((c) => c.args.id));
  await page.locator(".xterm-helper-textarea").last().focus();
  await page.keyboard.type("ls");
  await expect.poll(async () => (await ids()).size).toBe(2);
  await shot(page, "terminal-split-broadcast", info.project.name);

  await toggle.click();
  await expect(page.locator(".badge-bc")).toHaveCount(0);
  await page.locator(".xterm-helper-textarea").last().focus();
  const before = (await calls(page, "term_write")).length;
  await page.keyboard.type("x");
  await expect.poll(async () => (await calls(page, "term_write")).length).toBeGreaterThan(before);
  const after = (await calls(page, "term_write")).slice(before);
  expect(new Set(after.map((c) => c.args.id)).size).toBe(1);
});

test("enviar a todos: atajo de teclado", async ({ page }) => {
  await openWeb(page);
  const toggle = page.getByRole("button", { name: "Enviar a todos", exact: true });
  const mac = await page.evaluate(() => /Mac/.test(navigator.platform));
  await page.keyboard.press(mac ? "Meta+Shift+KeyB" : "Control+Shift+KeyB");
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
});

test("grabar sesión: REC, ruta y mostrar en el gestor de archivos", async ({ page }) => {
  await openWeb(page);
  await page.getByRole("button", { name: "Grabar sesión" }).click();
  const [start] = await calls(page, "term_log_start");
  await expect(page.locator(".rec")).toBeVisible();
  const path = page.locator(".logpath");
  await expect(path).toContainText("sesion-");
  await page.getByRole("button", { name: /^Mostrar/ }).click();
  expect((await calls(page, "reveal_path"))[0].args.path).toBe(await path.getAttribute("title"));
  expect(start.args.id).toBeGreaterThan(0);
  await page.getByRole("button", { name: "Detener grabación" }).click();
  expect(await calls(page, "term_log_stop")).toHaveLength(1);
  await expect(page.locator(".rec")).toHaveCount(0);
});

test("restaurar: ofrece las pestañas anteriores sin reconectar solo", async ({ page }) => {
  await openWeb(page);
  await page.reload();
  const banner = page.getByRole("region", { name: "Restaurar sesiones anteriores" });
  await expect(banner).toBeVisible();
  expect(await calls(page, "term_open")).toHaveLength(0);
  await banner.getByRole("button", { name: "Restaurar sesiones anteriores" }).click();
  await expect(page.getByRole("tab", { name: /web-01/ })).toBeVisible();
  expect((await calls(page, "term_open"))[0].args.target).toEqual({ kind: "saved", name: "web-01" });
  await expect(banner).toHaveCount(0);
});

test("pestañas: renombrar, duplicar, reordenar y cerrar las demás", async ({ page }) => {
  await openWeb(page);
  await page.getByRole("tab", { name: /web-01/ }).dblclick();
  await page.getByLabel("Nombre de la pestaña").fill("produccion");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("tab", { name: /produccion/ })).toBeVisible();

  await page.getByRole("tab", { name: /produccion/ }).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Duplicar pestaña" }).click();
  await expect(page.getByRole("tab")).toHaveCount(2);
  expect((await calls(page, "term_open"))[1].args.target).toEqual({ kind: "saved", name: "web-01" });

  const labels = () => page.getByRole("tab").allTextContents();
  await page.getByRole("tab").nth(1).dragTo(page.getByRole("tab").nth(0));
  await expect.poll(async () => (await labels())[0]).toBe("produccion");

  await page.getByRole("tab").nth(0).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Cerrar las demás" }).click();
  await page.getByRole("button", { name: "Cerrar" }).last().click(); // confirmación
  await expect(page.getByRole("tab")).toHaveCount(1);
});
