import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "alpha",
          include: ["src/shared.test.js"]
        }
      },
      {
        test: {
          name: "bravo",
          include: ["src/shared.test.js"]
        }
      }
    ]
  }
});
