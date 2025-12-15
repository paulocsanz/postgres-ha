import { Client } from "pg";
import { sleep } from "./retry";

export interface DatabaseConfig {
  connectionString?: string;
  host?: string;
  port?: number;
  database?: string;
  user?: string;
  password?: string;
  ssl?: boolean | { rejectUnauthorized: boolean };
}

export class DatabaseClient {
  private client: Client | null = null;
  private config: DatabaseConfig;

  constructor(config: DatabaseConfig | string) {
    if (typeof config === "string") {
      this.config = { connectionString: config };
    } else {
      this.config = config;
    }
  }

  async connect(retries: number = 5, delayMs: number = 5000): Promise<void> {
    let lastError: Error | null = null;

    for (let i = 0; i < retries; i++) {
      try {
        // Don't force SSL - let the connection string or server decide
        this.client = new Client({
          connectionString: this.config.connectionString,
          ...(!this.config.connectionString && this.config),
        });
        // Handle connection errors without crashing (for failover tests)
        this.client.on("error", (err) => {
          console.log(`Database connection error: ${err.message}`);
        });
        await this.client.connect();
        console.log("Database connected successfully");
        return;
      } catch (error) {
        lastError = error as Error;
        console.log(
          `Connection attempt ${i + 1}/${retries} failed: ${lastError.message}`
        );
        if (this.client) {
          try {
            await this.client.end();
          } catch {
            // Ignore cleanup errors
          }
          this.client = null;
        }
        if (i < retries - 1) {
          await sleep(delayMs);
        }
      }
    }

    throw new Error(
      `Failed to connect after ${retries} attempts: ${lastError?.message}`
    );
  }

  async disconnect(): Promise<void> {
    if (this.client) {
      await this.client.end();
      this.client = null;
    }
  }

  async reconnect(retries: number = 5, delayMs: number = 5000): Promise<void> {
    await this.disconnect();
    await this.connect(retries, delayMs);
  }

  async query<T = Record<string, unknown>>(
    sql: string,
    params?: unknown[]
  ): Promise<T[]> {
    if (!this.client) {
      throw new Error("Not connected to database");
    }
    const result = await this.client.query(sql, params);
    return result.rows as T[];
  }

  async createTestTable(): Promise<void> {
    await this.query(`
      CREATE TABLE IF NOT EXISTS failover_test (
        id SERIAL PRIMARY KEY,
        data TEXT NOT NULL,
        created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
      )
    `);
    console.log("Test table created");
  }

  async insertTestData(data: string): Promise<number> {
    const result = await this.query<{ id: number }>(
      "INSERT INTO failover_test (data) VALUES ($1) RETURNING id",
      [data]
    );
    console.log(`Test data inserted with ID: ${result[0].id}`);
    return result[0].id;
  }

  async verifyTestData(id: number, expectedData?: string): Promise<boolean> {
    const result = await this.query<{ data: string }>(
      "SELECT data FROM failover_test WHERE id = $1",
      [id]
    );

    if (result.length === 0) {
      return false;
    }

    if (expectedData !== undefined) {
      return result[0].data === expectedData;
    }

    return true;
  }

  async getCurrentServerAddress(): Promise<string | null> {
    try {
      const result = await this.query<{ inet_server_addr: string }>(
        "SELECT inet_server_addr()"
      );
      return result[0]?.inet_server_addr || null;
    } catch {
      return null;
    }
  }

  async isReadOnly(): Promise<boolean> {
    try {
      const result = await this.query<{ pg_is_in_recovery: boolean }>(
        "SELECT pg_is_in_recovery()"
      );
      return result[0]?.pg_is_in_recovery || false;
    } catch {
      return false;
    }
  }
}
