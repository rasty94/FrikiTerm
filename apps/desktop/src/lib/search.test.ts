import { expect, test } from "vitest";
import { filterHosts } from "./search";
import { defaultSession } from "./session";

const hosts = [
  defaultSession({ name: "Web Prod", host: "10.0.0.5", user: "deploy", folder: "prod" }),
  defaultSession({ name: "Router", host: "192.168.1.1", protocol: "telnet", folder: "lab" }),
];

test("consulta vacía devuelve todos", () => {
  expect(filterHosts(hosts, "  ")).toHaveLength(2);
});

test("busca sin distinguir mayúsculas en nombre, host, usuario, carpeta y protocolo", () => {
  expect(filterHosts(hosts, "WEB").map((h) => h.name)).toEqual(["Web Prod"]);
  expect(filterHosts(hosts, "deploy").map((h) => h.name)).toEqual(["Web Prod"]);
  expect(filterHosts(hosts, "telnet").map((h) => h.name)).toEqual(["Router"]);
  expect(filterHosts(hosts, "lab").map((h) => h.name)).toEqual(["Router"]);
});

test("cada palabra debe coincidir", () => {
  expect(filterHosts(hosts, "prod 10.0")).toHaveLength(1);
  expect(filterHosts(hosts, "prod 192")).toHaveLength(0);
});
