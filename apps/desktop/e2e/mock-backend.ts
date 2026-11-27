// Backend falso en memoria. Se inyecta ANTES de que cargue la app y define
// window.__TAURI_INTERNALS__ con el mismo protocolo que el real:
//  - transformCallback(cb) registra window["_<id>"] y devuelve el id numérico;
//  - invoke(cmd, args) recibe los Channel como objetos con `.id` (en el real se serializan a "__CHANNEL__:<id>");
//  - los mensajes de un Channel se entregan llamando a window["_<id>"]({ message, index }).
// Debe ser autocontenida: Playwright la serializa con toString().
export function installBackend(): void {
  const w = window as any;
  const calls: { cmd: string; args: any }[] = [];
  const unhandled: string[] = [];
  const callbacks = new Map<number, (m: any) => void>();
  let nextCb = 1;
  let nextId = 1;
  const indexes = new Map<number, number>();

  const cfg = (name: string, folder: string, extra: any = {}) => ({
    name, host: `${name}.example.com`, port: 22, user: "root", auth: ["agent"], identity_files: [],
    proxy_jump: null, legacy_algorithms: false, term: "xterm-256color", max_auth_attempts: 6,
    connect_timeout_secs: 15, keepalive_secs: 30, log: null,
    terminal: { emulation: "xterm", scrollback: 10000, backspace: "delete", theme: "default", keymap: {} },
    folder, protocol: "ssh",
    serial: { device: "", baud: 115200, data_bits: 8, parity: "none", stop_bits: 1, flow: "none" },
    local_command: null, forwards: [], forward_agent: false, require_strict_kex: true,
    pkcs11_provider: null, gssapi: false, gssapi_delegate: false, ...extra,
  });
  let hosts: any[] = [
    cfg("web-01", "produccion/web"),
    cfg("web-02", "produccion/web"),
    cfg("db-01", "produccion/db", { port: 2222, user: "admin" }),
    cfg("router-casa", ""),
  ];
  // Valores por defecto de src/lib/settings.ts.
  let settings: unknown = {
    theme: "system", fontFamily: "", fontSize: 13, cursorStyle: "block", cursorBlink: true,
    scrollback: 10000, copyOnSelect: false, rightClick: "menu", confirmClose: true,
  };
  let snippets = [
    { name: "uptime", command: "uptime" },
    { name: "disco", command: "df -h" },
  ];
  const keys = [
    { path: "/home/e2e/.ssh/id_ed25519.pub", algorithm: "ssh-ed25519", comment: "e2e@laptop", fingerprint: "SHA256:keyFingerprintE2E", encrypted: false },
    { path: "/home/e2e/.ssh/id_rsa.pub", algorithm: "ssh-rsa", comment: "", fingerprint: "SHA256:rsaFingerprintE2E", encrypted: true },
  ];
  const fwd = { id: 1, spec: { kind: "local", bind: "127.0.0.1", port: 8080, host: "intranet", host_port: 80 }, listen: "127.0.0.1:8080", active: true, error: null };
  const e = (name: string, kind: string, size = 0, p?: string) => ({ name, path: p ?? `./${name}`, kind, size, mtime: 1700000000, mode: 0o644 });
  const localFs: Record<string, any[]> = {
    "": [e("docs", "dir"), e("notas.txt", "file", 1234), e("foto.png", "file", 204800)],
    docs: [e("informe.pdf", "file", 99999, "./informe.pdf")],
  };
  const remoteFs: Record<string, any[]> = {
    "/": [e("var", "dir", 0, "/var"), e("etc", "dir", 0, "/etc"), e("leeme.md", "file", 42, "/leeme.md")],
    "/var": [e("log", "dir", 0, "/var/log")],
  };

  const push = (channelId: number, message: any) => {
    const index = indexes.get(channelId) ?? 0;
    indexes.set(channelId, index + 1);
    w["_" + channelId]({ message, index });
  };
  let appChannel: number | null = null;
  const chOf = (c: any) => (typeof c === "object" ? c.id : Number(String(c).replace("__CHANNEL__:", "")));

  const handlers: Record<string, (a: any) => any> = {
    hosts_list: () => hosts,
    host_save: ({ host, previousName }) => {
      hosts = hosts.filter((h) => h.name !== (previousName ?? host.name));
      hosts.push(host);
    },
    host_delete: ({ name }) => { hosts = hosts.filter((h) => h.name !== name); },
    hosts_import_ssh_config: () => ({ added: 0, warnings: [] }),
    app_subscribe: ({ events }) => { appChannel = chOf(events); },
    prompt_answer: () => null,
    term_open: ({ output, events }) => {
      const id = nextId++;
      const out = chOf(output), ev = chOf(events);
      setTimeout(() => {
        push(out, new Uint8Array([...("hola\r\n")].map((c) => c.charCodeAt(0))).buffer);
        push(ev, { type: "opened", conn: 1, warnings: [], negotiated: "chacha20-poly1305@openssh.com", forwards: [fwd] });
      }, 50);
      return id;
    },
    term_write: () => null, term_resize: () => null, term_close: () => null,
    conn_open: () => 1,
    conn_close: () => null,
    sftp_open: () => 7,
    local_root: () => "/home/e2e/Documents",
    local_pick_root: () => null,
    fs_list: ({ fs, path }) => (fs.kind === "local" ? localFs[path] : remoteFs[path]) ?? [],
    keys_list: () => keys,
    snippets_list: () => snippets,
    snippets_save: (a) => { snippets = a.snippets; },
    fwd_list: () => [fwd],
    settings_get: () => settings,
    settings_save: (a) => { settings = a.settings; },
    open_url: () => null,
    "plugin:event|listen": () => nextId++,
    "plugin:event|unlisten": () => null,
  };

  w.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
    transformCallback(cb: (m: any) => void, once = false) {
      const id = nextCb++;
      callbacks.set(id, cb);
      w["_" + id] = (m: any) => { if (once) callbacks.delete(id); return cb(m); };
      return id;
    },
    unregisterCallback(id: number) { callbacks.delete(id); delete w["_" + id]; },
    convertFileSrc: (p: string) => p,
    async invoke(cmd: string, args: any = {}) {
      calls.push({ cmd, args: JSON.parse(JSON.stringify(args, (_k, v) => (v && typeof v === "object" && typeof v.id === "number" && "onmessage" in v ? `__CHANNEL__:${v.id}` : v))) });
      const h = handlers[cmd];
      if (!h) { unhandled.push(cmd); throw { kind: "other", message: `comando sin mock: ${cmd}` }; }
      return h(args);
    },
  };

  w.__mock = {
    calls, unhandled,
    called: (cmd: string) => calls.filter((c) => c.cmd === cmd),
    emitApp: (ev: any) => push(appChannel!, ev),
  };
}
