/** What `GET /api/status` reports: the running build and a liveness flag. */
export type Status = {
  version: string;
  ok: boolean;
};

async function request<T>(path: string): Promise<T> {
  const response = await fetch(`/api${path}`);
  if (!response.ok)
    throw new Error(`${path}: ${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

export const api = {
  getStatus: () => request<Status>("/status"),
};

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Pure so the loading and loaded states are testable without a fetch mock. */
export function formatVersion(status: Status | null): string {
  return status ? `v${status.version}` : "connecting…";
}
