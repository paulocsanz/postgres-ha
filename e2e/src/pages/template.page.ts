import { Page } from "@playwright/test";

export class TemplatePage {
  constructor(private page: Page) {}

  async deployTemplate(templateUrl: string): Promise<string> {
    // Navigate to template page
    await this.page.goto(templateUrl);

    // Dismiss cookie consent if present
    try {
      await this.page.getByRole("button", { name: "Accept" }).click({ timeout: 3000 });
    } catch {
      // Cookie banner not present
    }

    // Click deploy button (might be a link styled as button)
    const deployBtn = this.page.getByText("Deploy Now");
    await deployBtn.waitFor({ state: "visible", timeout: 10000 });
    await deployBtn.click();

    // There's a confirmation page - click Deploy again
    await this.page.waitForURL(/\/template\//, { timeout: 10000 });
    const confirmDeploy = this.page.getByRole("button", { name: /deploy/i });
    await confirmDeploy.waitFor({ state: "visible", timeout: 10000 });
    await confirmDeploy.click();

    // Wait for project creation and redirect
    await this.page.waitForURL(/\/project\//, { timeout: 120000 });

    // Extract project ID from URL
    const url = this.page.url();
    const match = url.match(/\/project\/([a-zA-Z0-9-]+)/);

    if (!match) {
      throw new Error(`Failed to extract project ID from URL: ${url}`);
    }

    return match[1];
  }
}
