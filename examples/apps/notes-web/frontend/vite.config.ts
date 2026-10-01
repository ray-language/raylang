import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// In development the page comes from Vite (hot reload) and /api goes to the raylang program,
// which `ray dev` runs on PORT (8080 by default).
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: { '/api': `http://127.0.0.1:${process.env.PORT ?? '8080'}` },
  },
})
