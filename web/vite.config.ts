import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 开发期: Vite dev server 固定端口 1420（tauri.conf.json devUrl 引用）
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
