import { defineConfig, devices } from "@playwright/test";
import dotenv from "dotenv";

dotenv.config();

export default defineConfig({
  testDir: "./tests",
  timeout: parseInt(process.env.TEST_TIMEOUT || "600000"), // 10 minutes
  expect: {
    timeout: 30000,
  },
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: [["html"], ["json", { outputFile: "test-results/results.json" }]],
  use: {
    ...devices["Desktop Chrome"],
    baseURL: "https://railway.com",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
});
