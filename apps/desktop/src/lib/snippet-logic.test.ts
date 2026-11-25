import { expect, test } from "vitest";
import { snippetBytes, validateSnippets } from "./snippet-logic";

test("validateSnippets detecta vacíos y duplicados", () => {
  expect(validateSnippets([{ name: "a", command: "ls" }])).toBeNull();
  expect(validateSnippets([{ name: " ", command: "ls" }])).not.toBeNull();
  expect(validateSnippets([{ name: "a", command: "ls" }, { name: "a", command: "pwd" }])).toMatch(/duplicado/);
  expect(validateSnippets([{ name: "a", command: "  " }])).toMatch(/vacío/);
});

test("snippetBytes añade CR opcional", () => {
  expect(new TextDecoder().decode(snippetBytes("ls", true))).toBe("ls\r");
  expect(new TextDecoder().decode(snippetBytes("ls", false))).toBe("ls");
});
