import { describe, expect, it } from "vitest";
import { defaultSession } from "./session";
import { hasErrors, parseIdentityText, validateBaud, validateForward, validateSession } from "./hostForm";

const ssh = () => defaultSession({ name: "web", host: "10.0.0.1" });
const serial = () => defaultSession({ name: "consola", protocol: "serial", host: "" });

describe("validateSession: SSH", () => {
  it("un host completo es válido", () => {
    expect(hasErrors(validateSession(ssh()))).toBe(false);
  });

  it("exige nombre y host", () => {
    expect(validateSession({ ...ssh(), name: "  " }).name).toBeTruthy();
    expect(validateSession({ ...ssh(), host: "" }).host).toBeTruthy();
  });

  it("puerto entre 1 y 65535", () => {
    for (const port of [0, -1, 65536, 22.5, Number.NaN]) {
      expect(validateSession({ ...ssh(), port }).port, String(port)).toBeTruthy();
    }
    expect(validateSession({ ...ssh(), port: 1 }).port).toBeUndefined();
    expect(validateSession({ ...ssh(), port: 65535 }).port).toBeUndefined();
  });

  it("timeout y reintentos enteros positivos; keepalive admite 0 y null", () => {
    expect(validateSession({ ...ssh(), connect_timeout_secs: 0 }).timeout).toBeTruthy();
    expect(validateSession({ ...ssh(), max_auth_attempts: 0 }).maxAuth).toBeTruthy();
    expect(validateSession({ ...ssh(), keepalive_secs: -1 }).keepalive).toBeTruthy();
    expect(validateSession({ ...ssh(), keepalive_secs: 0 }).keepalive).toBeUndefined();
    expect(validateSession({ ...ssh(), keepalive_secs: null }).keepalive).toBeUndefined();
  });

  it("señala la primera redirección inválida", () => {
    const forwards = [
      { kind: "local" as const, bind: "127.0.0.1", port: 8080, host: "intranet", host_port: 80 },
      { kind: "dynamic" as const, bind: "127.0.0.1", port: 0 },
    ];
    const e = validateSession({ ...ssh(), forwards });
    expect(e.forwards).toContain("Redirección 2");
  });
});

describe("validateForward", () => {
  it("las dinámicas no piden destino", () => {
    expect(validateForward({ kind: "dynamic", bind: "127.0.0.1", port: 1080 })).toBeNull();
  });

  it("las locales y remotas exigen destino válido", () => {
    expect(validateForward({ kind: "remote", bind: "0.0.0.0", port: 9000, host: "", host_port: 22 })).toBeTruthy();
    expect(validateForward({ kind: "local", bind: "127.0.0.1", port: 9000, host: "h", host_port: 70000 })).toBeTruthy();
    expect(validateForward({ kind: "local", bind: "", port: 9000, host: "h", host_port: 22 })).toBeTruthy();
  });
});

describe("validateSession: protocolos sin red", () => {
  it("serie: exige dispositivo, velocidad entera positiva, bits 5-8 y parada 1-2", () => {
    const base = serial();
    expect(validateSession(base).device).toBeTruthy();
    const ok = { ...base, serial: { ...base.serial, device: "/dev/ttyUSB0" } };
    expect(hasErrors(validateSession(ok))).toBe(false);
    expect(validateSession({ ...ok, serial: { ...ok.serial, baud: 0 } }).baud).toBeTruthy();
    expect(validateSession({ ...ok, serial: { ...ok.serial, baud: 12345 } }).baud).toBeUndefined();
    expect(validateSession({ ...ok, serial: { ...ok.serial, data_bits: 9 } }).dataBits).toBeTruthy();
    expect(validateSession({ ...ok, serial: { ...ok.serial, stop_bits: 1.5 } }).stopBits).toBeTruthy();
  });

  it("shell local: solo el nombre", () => {
    const local = defaultSession({ name: "local", protocol: "local-shell", host: "", port: 0 });
    expect(hasErrors(validateSession(local))).toBe(false);
    expect(validateSession({ ...local, name: "" }).name).toBeTruthy();
  });

  it("telnet exige host y puerto", () => {
    const t = defaultSession({ name: "router", protocol: "telnet", host: "" });
    expect(validateSession(t).host).toBeTruthy();
    expect(validateSession({ ...t, host: "10.0.0.2", port: 23 })).toEqual({});
  });
});

describe("validateBaud", () => {
  it("acepta enteros positivos, incluidos los no estándar", () => {
    expect(validateBaud(250000)).toBeNull();
    expect(validateBaud(0)).toBeTruthy();
    expect(validateBaud(-9600)).toBeTruthy();
    expect(validateBaud(9600.5)).toBeTruthy();
  });
});

describe("parseIdentityText", () => {
  it("quita líneas vacías y espacios", () => {
    expect(parseIdentityText("  ~/.ssh/id_ed25519 \n\n ~/.ssh/otra\n")).toEqual(["~/.ssh/id_ed25519", "~/.ssh/otra"]);
  });
});
