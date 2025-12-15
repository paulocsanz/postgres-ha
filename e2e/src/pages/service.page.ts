import { Page } from "@playwright/test";

export class ServicePage {
  constructor(private page: Page) {}

  async getEnvironmentVariable(varName: string): Promise<string | null> {
    // Click on Variables tab in the service panel
    await this.page.locator("#stack-container-root").getByRole("link", { name: "Variables", exact: true }).click();
    await this.page.waitForTimeout(2000);

    // Look for the variable row - Railway shows variables as key=value rows
    const servicePanel = this.page.locator("#stack-container-root");

    // Try to find the variable by looking for its name
    const varRow = servicePanel.locator(`[data-testid="variable-row"]`).filter({ hasText: varName }).first();

    if (await varRow.isVisible({ timeout: 5000 }).catch(() => false)) {
      // Try to click on the row to reveal the value or find copy button
      const copyButton = varRow.locator('button').filter({ hasText: /copy/i }).first();
      if (await copyButton.isVisible({ timeout: 2000 }).catch(() => false)) {
        // Use clipboard to get value
        await copyButton.click();
        return await this.page.evaluate(() => navigator.clipboard.readText()).catch(() => null);
      }

      // Try to get value from input field
      const input = varRow.locator('input').first();
      if (await input.isVisible({ timeout: 2000 }).catch(() => false)) {
        return await input.inputValue().catch(() => null);
      }
    }

    // Fallback: look for any element containing the variable name and try to get its sibling value
    const varLabel = servicePanel.getByText(varName, { exact: true }).first();
    if (await varLabel.isVisible({ timeout: 2000 }).catch(() => false)) {
      // Look for a nearby input or value element
      const parent = varLabel.locator('xpath=ancestor::div[contains(@class, "row") or contains(@class, "variable")]').first();
      const valueInput = parent.locator('input').first();
      if (await valueInput.isVisible({ timeout: 1000 }).catch(() => false)) {
        return await valueInput.inputValue().catch(() => null);
      }
    }

    return null;
  }

  async enablePublicNetworkingAndGetDomain(port: number = 5432): Promise<string | null> {
    const servicePanel = this.page.locator("#stack-container-root");

    // Click on Settings tab in the service panel
    await servicePanel.getByRole("link", { name: "Settings", exact: true }).click();
    await this.page.waitForTimeout(2000);

    // Look for Networking section in the sidebar or scroll to it
    const networkingLink = servicePanel.getByRole("link", { name: "Networking", exact: true });
    if (await networkingLink.isVisible({ timeout: 3000 }).catch(() => false)) {
      await networkingLink.click();
      await this.page.waitForTimeout(1000);
    }

    // Click "Generate Domain" button if visible
    const generateDomainBtn = servicePanel.getByRole("button", { name: /generate.*domain/i });
    if (await generateDomainBtn.isVisible({ timeout: 5000 }).catch(() => false)) {
      console.log("Clicking Generate Domain button...");
      await generateDomainBtn.click();
      await this.page.waitForTimeout(3000);
    }

    // Check if there are pending changes to deploy (banner at top)
    // The banner shows "Apply X changes" with a Deploy button that has keyboard shortcut text
    const deployBtn = this.page.getByRole("button", { name: /deploy/i }).first();
    if (await deployBtn.isVisible({ timeout: 5000 }).catch(() => false)) {
      console.log("Deploying pending changes...");
      await deployBtn.click();
      // Wait for deployment to start and complete
      await this.page.waitForTimeout(60000);
    }

    // Domain patterns for Railway
    // For TCP services (PostgreSQL), the format is: xxx.proxy.rlwy.net:PORT
    // For HTTP services, the format is: xxx.up.railway.app
    // Prioritize TCP proxy format for database connections
    const tcpProxyPattern = /([a-z0-9-]+\.proxy\.rlwy\.net:\d+)/i;
    const httpDomainPattern = /([a-z0-9-]+\.up\.railway\.app)/i;

    // Wait and retry to get the domain (it may take time to be generated)
    for (let i = 0; i < 20; i++) {
      // Refresh the panel
      await servicePanel.getByRole("link", { name: "Settings", exact: true }).click();
      await this.page.waitForTimeout(2000);

      // Click Networking link if visible
      if (await networkingLink.isVisible({ timeout: 1000 }).catch(() => false)) {
        await networkingLink.click();
        await this.page.waitForTimeout(1000);
      }

      // Search for domain in the settings panel - prioritize TCP proxy
      const settingsText = await servicePanel.textContent() || "";

      // First try to find TCP proxy domain (for database connections)
      const tcpMatch = settingsText.match(tcpProxyPattern);
      if (tcpMatch) {
        console.log(`Found TCP proxy domain: ${tcpMatch[1]}`);
        return tcpMatch[1];
      }

      // Fallback to HTTP domain
      const httpMatch = settingsText.match(httpDomainPattern);
      if (httpMatch) {
        console.log(`Found HTTP domain: ${httpMatch[1]}`);
        return httpMatch[1];
      }

      // Check if domain is still being generated or deployment in progress
      if (settingsText.includes("will be generated") ||
          settingsText.includes("generating") ||
          settingsText.includes("Applying")) {
        console.log("Waiting for domain to be generated...");
        await this.page.waitForTimeout(5000);
        continue;
      }

      // Check for Generate Domain button again (in case it appeared)
      if (await generateDomainBtn.isVisible({ timeout: 1000 }).catch(() => false)) {
        console.log("Clicking Generate Domain button again...");
        await generateDomainBtn.click();
        await this.page.waitForTimeout(3000);
        continue;
      }

      break;
    }

    // Try to find in specific elements
    const domainElement = servicePanel.locator('input, [class*="domain"], [class*="url"], code, span').first();

    if (await domainElement.isVisible({ timeout: 3000 }).catch(() => false)) {
      const text = await domainElement.textContent() || "";
      const tcpMatch = text.match(tcpProxyPattern);
      if (tcpMatch) return tcpMatch[1];
      const httpMatch = text.match(httpDomainPattern);
      if (httpMatch) return httpMatch[1];
    }

    return null;
  }

  async getPublicUrl(): Promise<string | null> {
    // Click on Settings tab in the service panel
    await this.page.locator("#stack-container-root").getByRole("link", { name: "Settings", exact: true }).click();
    await this.page.waitForTimeout(1000);

    // Look for existing public domain
    const domainPattern = /[\w-]+\.(proxy\.rlwy\.net|up\.railway\.app)(:\d+)?/;
    const servicePanel = this.page.locator("#stack-container-root");
    const settingsText = await servicePanel.textContent() || "";
    const domainMatch = settingsText.match(domainPattern);

    if (domainMatch) {
      return domainMatch[0];
    }

    return null;
  }

  async getConnectionString(type: "public" | "private" = "public"): Promise<string | null> {
    // Click on Variables tab in the service panel
    await this.page.locator("#stack-container-root").getByRole("link", { name: "Variables", exact: true }).click();
    await this.page.waitForTimeout(1000);

    // Look for DATABASE_URL or connection string
    const connectionKey = type === "public" ? "DATABASE_PUBLIC_URL" : "DATABASE_URL";

    // Try to find and copy the connection string
    const connectionElement = this.page
      .locator(`text=${connectionKey}`)
      .first();

    if (await connectionElement.isVisible()) {
      // Click to reveal or copy
      await connectionElement.click();
      await this.page.waitForTimeout(500);
    }

    return null; // Will need to extract from UI
  }

  async removeDeployment() {
    // Open service menu (three dots)
    await this.page
      .locator('[data-testid="service-menu"], [class*="menu-trigger"]')
      .first()
      .click();

    // Click Remove or Delete
    await this.page
      .getByRole("menuitem", { name: /remove|delete/i })
      .click();

    // Confirm deletion if dialog appears
    const confirmButton = this.page.getByRole("button", { name: /confirm|delete|remove/i });
    if (await confirmButton.isVisible({ timeout: 5000 }).catch(() => false)) {
      await confirmButton.click();
    }

    await this.page.waitForTimeout(3000);
  }

  async restartDeployment() {
    // Click redeploy button
    await this.page
      .getByRole("button", { name: /redeploy|restart/i })
      .click();

    // Confirm if needed
    const confirmButton = this.page.getByRole("button", { name: /confirm/i });
    if (await confirmButton.isVisible({ timeout: 3000 }).catch(() => false)) {
      await confirmButton.click();
    }

    await this.page.waitForTimeout(3000);
  }
}
