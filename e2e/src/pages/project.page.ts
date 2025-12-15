import { Page, Locator } from "@playwright/test";

export interface ServiceInfo {
  name: string;
  element: Locator;
}

export class ProjectPage {
  constructor(private page: Page) {}

  async goto(projectId: string) {
    await this.page.goto(`/project/${projectId}`);
  }

  async waitForServicesReady(
    expectedCount: number,
    timeout: number = 300000
  ): Promise<void> {
    const startTime = Date.now();

    // Wait for page to load
    await this.page.waitForLoadState("networkidle", { timeout: 30000 }).catch(() => {});

    while (Date.now() - startTime < timeout) {
      // Look for service nodes in the canvas
      const services = await this.page
        .getByTestId("service-node")
        .all();

      if (services.length >= expectedCount) {
        // Wait a bit more to ensure services are fully deployed
        await this.page.waitForTimeout(30000);
        return;
      }

      console.log(
        `Found ${services.length}/${expectedCount} services, waiting...`
      );
      await this.page.waitForTimeout(10000);
      await this.page.reload();
      await this.page.waitForLoadState("networkidle", { timeout: 30000 }).catch(() => {});
    }

    throw new Error(
      `Services did not reach ready state within ${timeout}ms`
    );
  }

  async getServices(): Promise<ServiceInfo[]> {
    const serviceElements = await this.page
      .getByTestId("service-node")
      .all();

    const services: ServiceInfo[] = [];
    for (const element of serviceElements) {
      const name = (await element.textContent()) || "";
      services.push({ name, element });
    }

    return services;
  }

  async clickService(serviceName: string) {
    // Close any existing service panel first
    const closeButton = this.page.locator("#stack-container-root").locator('button[aria-label="Close"], [class*="close"]').first();
    if (await closeButton.isVisible({ timeout: 1000 }).catch(() => false)) {
      await closeButton.click();
      await this.page.waitForTimeout(500);
    }

    // Click on the service by finding text that matches exactly
    const service = this.page
      .getByTestId("service-node")
      .filter({ hasText: new RegExp(`^${serviceName}`, 'i') })
      .first();

    console.log(`Clicking on service: ${serviceName}`);
    await service.click();
    await this.page.waitForTimeout(1000);
  }

  async openServiceSettings(serviceName: string) {
    await this.clickService(serviceName);

    // Service panel should open on the right - wait for the pane
    const servicePanel = this.page.locator("#stack-container-root");
    await servicePanel.waitFor({ state: "visible", timeout: 10000 });

    // Verify the correct service is open by checking the title
    const panelTitle = servicePanel.locator('h1, h2, [class*="title"]').first();
    await panelTitle.waitFor({ state: "visible", timeout: 5000 });
    const titleText = await panelTitle.textContent();

    if (!titleText?.toLowerCase().includes(serviceName.toLowerCase())) {
      console.log(`Warning: Expected panel for ${serviceName}, but got: ${titleText}`);
      // Try clicking again
      await this.clickService(serviceName);
      await this.page.waitForTimeout(2000);
    }

    console.log(`Opened service panel for: ${serviceName}`);
  }
}
