import { defineConfig } from "@playwright/test";

// Sirve el build de producción (no `vite dev`): así se prueba lo que se empaqueta.
export default defineConfig({
  testDir: "e2e",
  timeout: 30_000,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: "http://127.0.0.1:4173",
    // Sin WebGL xterm usa el renderer DOM (la app ya lo prevé) y el texto es aserible.
    launchOptions: { args: ["--disable-webgl", "--disable-webgl2"] },
  },
  projects: [
    { name: "light", use: { colorScheme: "light" } },
    { name: "dark", use: { colorScheme: "dark" } },
  ],
  webServer: {
    command: "npm run build && npx vite preview --host 127.0.0.1 --port 4173 --strictPort",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
