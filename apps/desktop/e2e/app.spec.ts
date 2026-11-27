import { calls, expect, shot, test } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
});

const nav = (page: any, name: string) => page.getByRole("navigation", { name: "Principal" }).getByRole("button", { name });

test("arranque: monta, rail, hosts, carpetas y búsqueda", async ({ page }, info) => {
  const rail = page.getByRole("navigation", { name: "Principal" });
  await expect(rail).toBeVisible();
  for (const n of ["Hosts", "Terminal", "SFTP", "Llavero", "Snippets", "Redirecciones"]) {
    await expect(nav(page, n)).toBeVisible();
  }
  for (const n of ["web-01", "web-02", "db-01", "router-casa"]) {
    await expect(page.getByTitle(`Conectar a ${n}`)).toBeVisible();
  }
  await expect(page.getByRole("button", { name: /Todos los hosts/ })).toContainText("4");
  await expect(page.getByText("produccion", { exact: false }).first()).toBeVisible();
  await shot(page, "hosts", info.project.name);

  await page.getByRole("searchbox", { name: "Buscar hosts" }).fill("db-");
  await expect(page.getByTitle("Conectar a db-01")).toBeVisible();
  await expect(page.getByTitle("Conectar a web-01")).toHaveCount(0);
  await page.getByRole("searchbox", { name: "Buscar hosts" }).fill("zzz");
  await expect(page.getByText("Sin resultados")).toBeVisible();
});

test("formulario de host: guardar envía un SessionConfig válido", async ({ page }) => {
  await page.getByRole("button", { name: "Nuevo host" }).click();
  const dlg = page.getByRole("dialog");
  await dlg.getByLabel("Nombre").fill("nuevo-srv");
  await dlg.getByLabel("Host", { exact: true }).fill("10.0.0.9");
  await dlg.getByRole("button", { name: "Guardar" }).click();
  await expect(dlg).toHaveCount(0);
  const [c] = await calls(page, "host_save");
  expect(c.args.previousName).toBeNull();
  expect(c.args.host).toMatchObject({
    name: "nuevo-srv", host: "10.0.0.9", port: 22, protocol: "ssh", folder: "", forwards: [], log: null,
    terminal: { emulation: expect.any(String), scrollback: expect.any(Number) },
    serial: { baud: expect.any(Number) },
  });
  expect(Array.isArray(c.args.host.auth) && Array.isArray(c.args.host.identity_files)).toBe(true);
  await expect(page.getByTitle("Conectar a nuevo-srv")).toBeVisible();
});

test("terminal: abre pestaña, pinta la salida y muestra el cifrado", async ({ page }, info) => {
  await page.getByTitle("Conectar a web-01").click();
  await expect(page.getByRole("tab", { name: "web-01" })).toBeVisible();
  await expect(page.locator(".xterm")).toBeVisible();
  await expect(page.locator(".xterm-rows")).toContainText("hola");
  await expect(page.getByText("Negociado: chacha20-poly1305@openssh.com")).toBeVisible();
  const [c] = await calls(page, "term_open");
  expect(c.args.target).toEqual({ kind: "saved", name: "web-01" });
  await shot(page, "terminal", info.project.name);
});

test("prompt: huella de host y respuesta acceptAndSave", async ({ page }) => {
  await page.evaluate(() =>
    (window as any).__mock.emitApp({
      type: "prompt", id: 42,
      prompt: { kind: "hostKeyUnknown", host: "web-01.example.com", port: 22, algorithm: "ssh-ed25519", fingerprint: "SHA256:abcDEF123" },
    }),
  );
  const dlg = page.getByRole("dialog", { name: "Host desconocido" });
  await expect(dlg).toBeVisible();
  await expect(dlg).toContainText("SHA256:abcDEF123");
  await dlg.getByRole("button", { name: "Aceptar y guardar" }).click();
  await expect(dlg).toHaveCount(0);
  const [c] = await calls(page, "prompt_answer");
  expect(c.args).toEqual({ id: 42, answer: { kind: "hostKey", decision: "acceptAndSave" } });
});

test("sftp: panel local relativo a la raíz y panel remoto", async ({ page }, info) => {
  await nav(page, "SFTP").click();
  const local = page.getByRole("region", { name: "Local" });
  await expect(local.getByRole("row", { name: /notas\.txt/ })).toBeVisible();
  await expect(local.getByRole("button", { name: "Documents" })).toBeVisible(); // migas: raíz
  expect((await calls(page, "fs_list"))[0].args).toMatchObject({ fs: { kind: "local", root: "/home/e2e/Documents" }, path: "" });
  await local.getByRole("row", { name: /docs/ }).dblclick();
  await expect(local.getByRole("button", { name: "docs", exact: true })).toBeVisible();
  await expect(local.getByRole("row", { name: /informe\.pdf/ })).toBeVisible();
  expect((await calls(page, "fs_list")).at(-1)!.args.path).toBe("docs");

  await page.getByRole("combobox", { name: "Host remoto" }).selectOption("web-01");
  const remote = page.getByRole("region", { name: "Remoto · web-01" });
  await expect(remote.getByRole("row", { name: /leeme\.md/ })).toBeVisible();
  await remote.getByRole("row", { name: /var/ }).dblclick();
  await expect(remote.getByRole("button", { name: "log" })).toHaveCount(0);
  await expect(remote.getByRole("row", { name: /log/ })).toBeVisible();
  await shot(page, "sftp", info.project.name);
});

test("llavero, snippets y redirecciones muestran los datos", async ({ page }, info) => {
  await nav(page, "Llavero").click();
  await expect(page.getByText("SHA256:keyFingerprintE2E")).toBeVisible();
  await expect(page.getByText("cifrada", { exact: true })).toBeVisible();
  await shot(page, "keychain", info.project.name);

  await nav(page, "Snippets").click();
  await expect(page.getByRole("button", { name: "uptime" })).toBeVisible();
  await expect(page.getByRole("button", { name: "disco" })).toBeVisible();
  await page.getByRole("button", { name: "disco" }).click();
  await expect(page.locator("textarea")).toHaveValue("df -h");
  await page.getByRole("button", { name: "Guardar" }).click();
  expect((await calls(page, "snippets_save"))[0].args.snippets).toEqual([
    { name: "uptime", command: "uptime" }, { name: "disco", command: "df -h" },
  ]);
  await shot(page, "snippets", info.project.name);

  // Las redirecciones necesitan una conexión abierta: se crea con una terminal.
  await nav(page, "Hosts").click();
  await page.getByTitle("Conectar a web-01").click();
  await expect(page.getByText("Negociado:")).toBeVisible();
  await nav(page, "Redirecciones").click();
  await expect(page.getByText("127.0.0.1:8080").first()).toBeVisible();
  await shot(page, "forwards", info.project.name);
});

test("ajustes: tema segmentado y atajos", async ({ page }, info) => {
  await nav(page, "Ajustes").click();
  await expect(page.getByRole("radiogroup", { name: "Tema" })).toBeVisible();
  await expect(page.getByRole("radio", { name: "Sistema" })).toHaveAttribute("aria-checked", "true");
  await shot(page, "settings", info.project.name);
});

test("hosts: vista en cuadrícula persistida, recientes y teclado", async ({ page }, info) => {
  // Recientes: los 3 con última conexión, el más reciente primero; router-casa (nunca usado) no entra.
  const recents = page.getByRole("list", { name: "Recientes" });
  await expect(page.getByRole("heading", { name: "Recientes" })).toBeVisible();
  await expect(recents.locator("[data-nav] .name")).toHaveText(["web-01", "db-01", "web-02"]);
  await expect(recents.locator("[data-nav]")).toHaveCount(3);

  await page.getByRole("button", { name: "Vista de cuadrícula" }).click();
  await expect(page.getByRole("button", { name: "Vista de cuadrícula" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("ul.grid").first()).toBeVisible();
  await expect(page.getByRole("img", { name: "Debian" }).first()).toBeVisible();
  await shot(page, "hosts-grid", info.project.name);

  // Persistencia tras recargar.
  await page.reload();
  await expect(page.getByRole("button", { name: "Vista de cuadrícula" })).toHaveAttribute("aria-pressed", "true");

  // Con filtro no hay sección Recientes.
  await page.getByRole("searchbox", { name: "Buscar hosts" }).fill("web");
  await expect(page.getByRole("heading", { name: "Recientes" })).toHaveCount(0);
  await page.getByRole("searchbox", { name: "Buscar hosts" }).fill("");

  // "/" enfoca el buscador; ↓ salta al primer host; → mueve; Enter conecta.
  await page.getByRole("heading", { name: "Hosts", level: 1 }).click(); // saca el foco del buscador
  await page.keyboard.press("/");
  await expect(page.getByRole("searchbox", { name: "Buscar hosts" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(recents.locator("[data-nav]").first()).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect(recents.locator("[data-nav]").nth(1)).toBeFocused();
  await page.keyboard.press("Control+e");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Vista de lista" }).click();
  await page.getByRole("list", { name: "Hosts" }).getByTitle("Conectar a router-casa").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("tab", { name: "router-casa" })).toBeVisible();
});

test("formulario: host serie con puerto de la lista y velocidad", async ({ page }) => {
  await page.getByRole("button", { name: "Nuevo host" }).click();
  const dlg = page.getByRole("dialog");
  await dlg.getByLabel("Nombre").fill("consola-serie");
  await dlg.getByLabel("Protocolo").selectOption("serial");
  await expect(dlg.getByLabel("Host", { exact: true })).toHaveCount(0);
  await dlg.getByRole("tab", { name: "Serie" }).click();
  await expect(dlg.getByRole("option", { name: /ttyUSB0 · USB/ })).toHaveCount(1);
  await dlg.getByLabel("Dispositivo").selectOption("/dev/ttyUSB0");
  await dlg.getByLabel("Velocidad (baud)").fill("250000");
  await dlg.getByRole("button", { name: "Guardar" }).click();
  await expect(dlg).toHaveCount(0);
  const [c] = await calls(page, "host_save");
  expect(c.args.host).toMatchObject({
    name: "consola-serie", protocol: "serial", port: 22,
    serial: { device: "/dev/ttyUSB0", baud: 250000, data_bits: 8, parity: "none", stop_bits: 1, flow: "none" },
  });
  await expect(page.getByTitle("Conectar a consola-serie")).toBeVisible();
});

test("formulario: host SSH con archivo de identidad elegido y validación del puerto", async ({ page }, info) => {
  await page.getByRole("button", { name: "Nuevo host" }).click();
  const dlg = page.getByRole("dialog");
  await dlg.getByLabel("Nombre").fill("ssh-clave");
  await dlg.getByLabel("Host", { exact: true }).fill("10.0.0.8");
  const guardar = dlg.getByRole("button", { name: "Guardar" });
  await expect(guardar).toBeEnabled();

  await dlg.getByLabel("Puerto", { exact: true }).fill("70000");
  await expect(dlg.getByText("Puerto entre 1 y 65535")).toBeVisible();
  await expect(guardar).toBeDisabled();
  await dlg.getByLabel("Puerto", { exact: true }).fill("2222");
  await expect(guardar).toBeEnabled();

  await dlg.getByRole("tab", { name: "Autenticación" }).click();
  await dlg.getByRole("button", { name: "Elegir archivo…" }).click();
  await expect(dlg.getByLabel("Archivo de identidad 1")).toHaveValue("/home/e2e/.ssh/id_ed25519");
  await shot(page, "host-ssh", info.project.name);
  await guardar.click();
  await expect(dlg).toHaveCount(0);
  const [c] = await calls(page, "host_save");
  expect(c.args.host).toMatchObject({ name: "ssh-clave", host: "10.0.0.8", port: 2222, protocol: "ssh", identity_files: ["/home/e2e/.ssh/id_ed25519"] });
});

test("menú contextual: iconos, teclado y peligro; tema rápido sincronizado", async ({ page }) => {
  await page.getByTitle("Conectar a web-01").click({ button: "right" });
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem")).toHaveCount(5);
  await expect(menu.locator(".ico svg")).toHaveCount(5);
  await expect(menu.getByRole("menuitem", { name: "Conectar" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: "Abrir SFTP" })).toBeFocused();
  await page.keyboard.press("End");
  await expect(menu.getByRole("menuitem", { name: "Eliminar" })).toBeFocused();
  await expect(menu.getByRole("menuitem", { name: "Eliminar" })).toHaveClass(/danger/);
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: "Conectar" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);

  await page.getByRole("button", { name: /Tema: Sistema/ }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByRole("button", { name: /Tema: Claro/ }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  expect((await calls(page, "settings_save")).at(-1)!.args.settings.theme).toBe("dark");
  await nav(page, "Ajustes").click();
  await expect(page.getByRole("radio", { name: "Oscuro" })).toHaveAttribute("aria-checked", "true");
});

for (const [w, h] of [[1024, 700], [860, 640]] as const) {
  test(`responsive ${w}x${h}: rail de iconos y sin scroll horizontal`, async ({ page }, info) => {
    await page.setViewportSize({ width: w, height: h });
    const rail = page.getByRole("navigation", { name: "Principal" });
    await expect(rail).toBeVisible();
    if (w < 1100) {
      // Solo iconos: el texto se oculta, el nombre sigue accesible.
      await expect(page.locator(".item .label").first()).toBeHidden();
      expect((await rail.boundingBox())!.width).toBeLessThanOrEqual(60);
    }
    await expect(nav(page, "Llavero")).toBeVisible();
    for (const view of ["Hosts", "SFTP", "Llavero", "Snippets", "Redirecciones", "Ajustes"]) {
      await nav(page, view).click();
      const overflow = await page.evaluate(() => {
        const m = document.querySelector("main")!;
        return { main: m.scrollWidth - m.clientWidth, doc: document.documentElement.scrollWidth - innerWidth };
      });
      expect(overflow, view).toEqual({ main: 0, doc: 0 });
    }
    await shot(page, `responsive-${w}`, info.project.name);
  });
}
