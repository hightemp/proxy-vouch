export type Protocol =
  "auto" | "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h";
export type Status =
  | "Unchecked"
  | "Queued"
  | "Checking"
  | "Working"
  | "Failed"
  | "Inconclusive"
  | "Cancelled"
  | "Invalid";
export interface AppError {
  code: string;
  message: string;
}
export interface Attempt {
  protocol: Protocol;
  detected: Protocol | null;
  status: Status;
  authentication: string;
  code: string;
  stage: string;
  message: string;
  durationMs: number;
  exitIp: string | null;
  countryCode: string | null;
  checkUrl: string;
}
export interface AnonymityResult {
  level: "Transparent" | "Anonymous" | "Elite" | "Unknown";
  message: string;
  checkUrl: string;
  observedIp: string | null;
  proxyHeaders: string[];
}
export interface CheckResult {
  status: Status;
  detected: Protocol | null;
  authentication: string;
  latencyMs: number | null;
  totalDurationMs: number;
  exitIp: string | null;
  countryCode: string | null;
  anonymity: AnonymityResult | null;
  speed: SpeedResult | null;
  checkedAt: string;
  code: string;
  stage: string;
  message: string;
  checkUrl: string;
  attempts: Attempt[];
}
export interface SpeedResult {
  outcome: "Completed" | "Partial" | "Failed" | "Cancelled" | "Skipped";
  downloadMbps: number | null;
  requestedBytes: number;
  receivedBytes: number;
  durationMs: number;
  limit: "NotObserved" | "Signaled" | "Unknown";
  httpStatus: number | null;
  proxyHttpStatus: number | null;
  checkUrl: string;
  message: string;
}
export interface Row {
  id: number;
  address: string;
  host: string;
  port: number | null;
  username: string;
  hasCredentials: boolean;
  requestedProtocol: Protocol;
  protocol: Protocol;
  status: Status;
  label: string;
  source: string;
  line: number;
  error: AppError | null;
  result: CheckResult | null;
}
export interface Snapshot {
  revision: number;
  reset: boolean;
  rows: Row[];
  running: boolean;
  runId: number;
  scheduled: number;
  completed: number;
  total: number;
  counts: Partial<Record<Status, number>>;
}
export interface Preview {
  rows: Row[];
  valid: number;
  invalid: number;
  duplicates: number;
  ignored: number;
  total: number;
}
export interface Settings {
  url: string;
  fallbackUrl: string;
  ipEcho: boolean;
  countryLookup: boolean;
  anonymityCheck: boolean;
  speedCheck: boolean;
  speedTestMib: number;
  expectedStatus: number;
  bodyContains: string;
  concurrency: number;
  rateLimit: number;
  connectTimeoutMs: number;
  attemptTimeoutMs: number;
  totalTimeoutMs: number;
  retries: number;
}
export interface Preferences {
  theme: string;
  check: Settings;
}
export interface StorageStatus {
  directory: string;
  savedRevision: number | null;
  error: AppError | null;
  notice: string | null;
}
export interface BackupPreview {
  sourceName: string;
  summary: {
    createdAt: string;
    proxies: number | null;
    invalid: number;
    results: number;
    hasSettings: boolean;
    hasCredentials: boolean;
  };
}
export interface ImportOptions {
  format: string;
  delimiter: string;
  header: string;
  columns: string[];
  sourceName: string;
}
export interface ExportOptions {
  scope: string;
  format: string;
  credentials: boolean;
  ids: number[];
}
export const defaultSettings: Settings = {
  url: "https://api64.ipify.org?format=json",
  fallbackUrl: "",
  ipEcho: true,
  countryLookup: true,
  anonymityCheck: true,
  speedCheck: true,
  speedTestMib: 1,
  expectedStatus: 200,
  bodyContains: "",
  concurrency: 20,
  rateLimit: 10,
  connectTimeoutMs: 3000,
  attemptTimeoutMs: 8000,
  totalTimeoutMs: 45000,
  retries: 0,
};
