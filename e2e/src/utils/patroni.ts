import { request } from "@playwright/test";
import { poll, sleep } from "./retry";

export interface PatroniMember {
  name: string;
  role: "leader" | "replica" | "standby_leader" | "sync_standby";
  state: string;
  timeline?: number;
  lag?: number;
  api_url?: string;
  host?: string;
}

export interface PatroniClusterStatus {
  members: PatroniMember[];
  scope?: string;
}

export class PatroniClient {
  constructor(private baseUrls: string[]) {}

  async getClusterStatus(): Promise<PatroniClusterStatus | null> {
    for (const baseUrl of this.baseUrls) {
      try {
        const apiContext = await request.newContext();
        const response = await apiContext.get(`${baseUrl}/cluster`, {
          timeout: 10000,
        });

        if (response.ok()) {
          const data = await response.json();
          await apiContext.dispose();
          return data;
        }
        await apiContext.dispose();
      } catch (error) {
        console.log(`Failed to reach Patroni at ${baseUrl}: ${error}`);
        continue;
      }
    }
    return null;
  }

  async getPrimaryNode(): Promise<PatroniMember | null> {
    const status = await this.getClusterStatus();
    if (!status) return null;

    return status.members.find((m) => m.role === "leader") || null;
  }

  async getReplicaNodes(): Promise<PatroniMember[]> {
    const status = await this.getClusterStatus();
    if (!status) return [];

    return status.members.filter(
      (m) => m.role === "replica" || m.role === "sync_standby"
    );
  }

  async isPrimary(endpoint: string): Promise<boolean> {
    try {
      const apiContext = await request.newContext();
      const response = await apiContext.get(`${endpoint}/primary`, {
        timeout: 5000,
      });
      const result = response.status() === 200;
      await apiContext.dispose();
      return result;
    } catch {
      return false;
    }
  }

  async waitForNewPrimary(
    excludeNode: string,
    timeoutMs: number = 60000,
    pollIntervalMs: number = 2000
  ): Promise<PatroniMember | null> {
    console.log(
      `Waiting for new primary (excluding ${excludeNode}) with timeout ${timeoutMs}ms`
    );

    try {
      return await poll(
        async () => {
          const primary = await this.getPrimaryNode();
          return primary;
        },
        (primary) => {
          if (!primary) return false;
          if (primary.name === excludeNode) return false;
          if (primary.state !== "running") return false;
          console.log(`Found new primary: ${primary.name} (${primary.state})`);
          return true;
        },
        timeoutMs,
        pollIntervalMs
      );
    } catch (error) {
      console.log(`Timed out waiting for new primary: ${error}`);
      return null;
    }
  }

  async waitForClusterHealthy(
    minMembers: number = 2,
    timeoutMs: number = 60000,
    pollIntervalMs: number = 3000
  ): Promise<boolean> {
    console.log(
      `Waiting for cluster to be healthy with at least ${minMembers} members`
    );

    try {
      await poll(
        async () => {
          const status = await this.getClusterStatus();
          return status;
        },
        (status) => {
          if (!status) return false;
          const healthyMembers = status.members.filter(
            (m) => m.state === "running" || m.state === "streaming"
          );
          const leaders = status.members.filter((m) => m.role === "leader");

          console.log(
            `Cluster status: ${healthyMembers.length} healthy members, ${leaders.length} leader(s)`
          );

          return healthyMembers.length >= minMembers && leaders.length === 1;
        },
        timeoutMs,
        pollIntervalMs
      );
      return true;
    } catch {
      return false;
    }
  }
}
