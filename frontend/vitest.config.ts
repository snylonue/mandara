import { mergeConfig } from "vite";
import { defineConfig } from "vitest/config";
import viteConfig from "./vite.config.ts";

// The Nix shell may export NODE_ENV=production; React tests need act().
process.env.NODE_ENV = "test";

export default mergeConfig(viteConfig, defineConfig({
  test: {
    environment: "jsdom",
    css: { include: [/epub-(image-page|content)\.css/, /noto-serif-sc/] },
    env: { NODE_ENV: "test" },
    include: ["tests/**/*.test.tsx"],
    setupFiles: ["./tests/setup.ts"],
  },
}));
