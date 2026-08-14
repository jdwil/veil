
export interface ApiClient {
  fetch(endpoint: string, params: Record<string, unknown>): Promise<Record<string, unknown>>;
  mutate(endpoint: string, body: Record<string, unknown>): Promise<Record<string, unknown>>;
  put(endpoint: string, body: Record<string, unknown>): Promise<Record<string, unknown>>;
  delete(endpoint: string): Promise<Record<string, unknown>>;
}

