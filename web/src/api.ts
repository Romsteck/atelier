export interface AppCard {
  app_id: string;
  name: string;
  stack: string;
  description: string;
  logo: string;
  schema_version: number;
  has_overview: boolean;
  stats: {
    has_overview: boolean;
    screens: number;
    features: number;
    components: number;
    with_diagram: number;
  };
}

export interface OverviewEntry {
  doc_type: string;
  name: string;
  title: string;
  summary: string | null;
  scope: string | null;
  parent_screen: string | null;
  has_diagram: boolean;
}

export interface Overview {
  meta: AppCard;
  body: string;
  index: {
    screens: OverviewEntry[];
    features: OverviewEntry[];
    components: OverviewEntry[];
  };
  stats: AppCard["stats"];
}

export interface DocEntry {
  app_id: string;
  type: string;
  name: string;
  frontmatter: Record<string, unknown>;
  body: string;
  diagram: string | null;
}

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(path);
  const data = await res.json();
  if (!data.success) throw new Error(data.error ?? "request failed");
  return data;
}

// ─── Store ─────────────────────────────────────────────────────

export interface StoreAppSummary {
  slug: string;
  name: string;
  description: string;
  category: string;
  icon: string | null;
  android_package: string | null;
  publisher_app_id: string;
  latest_version: string | null;
  latest_size_bytes: number | null;
  release_count: number;
  created_at: string;
  updated_at: string;
}

export interface StoreRelease {
  version: string;
  changelog: string;
  sha256: string;
  size_bytes: number;
  created_at: string;
}

export interface StoreApp extends StoreAppSummary {
  releases: StoreRelease[];
}

export async function listStoreApps(): Promise<StoreAppSummary[]> {
  const data = await getJson<{ apps: StoreAppSummary[] }>("/api/store/apps");
  return data.apps;
}

export async function getStoreApp(slug: string): Promise<StoreApp> {
  const data = await getJson<{ app: StoreApp }>(`/api/store/apps/${slug}`);
  return data.app;
}

// ─── Dataverse ─────────────────────────────────────────────────

export interface DvColumn {
  name: string;
  pg_type: string;
  nullable: boolean;
  default?: string | null;
  is_primary_key?: boolean;
  is_system?: boolean;
}

export interface DvTable {
  name: string;
  columns: DvColumn[];
  primary_key?: string;
}

export interface DvSchema {
  version: number;
  updated_at: string;
  tables: DvTable[];
  relations: unknown[];
}

export interface DvListResult {
  value: Record<string, unknown>[];
  "@count"?: number;
}

async function dvFetch<T>(path: string, token: string | null): Promise<T> {
  const headers: HeadersInit = {};
  if (token) headers["Authorization"] = `Bearer ${token}`;
  const res = await fetch(path, { headers });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`HTTP ${res.status}: ${body.slice(0, 200)}`);
  }
  return res.json();
}

export function getDvSchema(slug: string, token: string): Promise<DvSchema> {
  return dvFetch(`/api/dv/${slug}/$schema`, token);
}

export function listDvRows(
  slug: string,
  table: string,
  token: string,
  opts: { top?: number; skip?: number; orderby?: string; filter?: string; count?: boolean } = {},
): Promise<DvListResult> {
  const params = new URLSearchParams();
  if (opts.top != null) params.set("$top", String(opts.top));
  if (opts.skip != null) params.set("$skip", String(opts.skip));
  if (opts.orderby) params.set("$orderby", opts.orderby);
  if (opts.filter) params.set("$filter", opts.filter);
  if (opts.count) params.set("$count", "true");
  const qs = params.toString();
  return dvFetch(`/api/dv/${slug}/${table}${qs ? "?" + qs : ""}`, token);
}

// ─── Apps ──────────────────────────────────────────────────────

export interface App {
  slug: string;
  name: string;
  stack: string;
  has_db: boolean;
  visibility: "public" | "private";
  domain: string;
  port: number;
  run_command: string;
  build_command: string;
  build_artefact: string;
  health_path: string;
  env_vars: Record<string, string>;
  state: string;
  sources_on: string | null;
  db_backend: string | null;
  created_at: string;
  updated_at: string;
}

export async function listApps(): Promise<App[]> {
  const data = await getJson<{ data: { apps: App[] } }>("/api/apps");
  return data.data.apps;
}

export async function getApp(slug: string): Promise<App> {
  const data = await getJson<{ data: App }>(`/api/apps/${slug}`);
  return data.data;
}

// ─── Git ───────────────────────────────────────────────────────

export interface GitMirror {
  github_url: string;
  enabled: boolean;
  last_sync_at: string | null;
  last_sync_status: string | null;
}

export interface GitRepo {
  slug: string;
  name: string;
  default_branch: string | null;
  size_bytes: number;
  branch_count: number;
  commit_count: number;
  last_commit_at: string | null;
  last_commit_message: string | null;
  visibility: string;
  mirror: GitMirror | null;
}

export interface GitCommit {
  sha: string;
  author_name: string;
  author_email: string;
  date: string;
  message: string;
}

export interface GitBranch {
  name: string;
  sha: string;
  is_default: boolean;
}

export async function listRepos(): Promise<GitRepo[]> {
  const data = await getJson<{ repos: GitRepo[] }>("/api/git/repos");
  return data.repos ?? [];
}

export async function getRepo(slug: string): Promise<GitRepo> {
  const data = await getJson<{ repo: GitRepo }>(`/api/git/repos/${slug}`);
  return data.repo;
}

export async function getCommits(slug: string, limit = 50): Promise<GitCommit[]> {
  const res = await fetch(`/api/git/repos/${slug}/commits?limit=${limit}`);
  const data = await res.json();
  return data.commits ?? [];
}

export async function getBranches(slug: string): Promise<GitBranch[]> {
  const res = await fetch(`/api/git/repos/${slug}/branches`);
  const data = await res.json();
  return data.branches ?? [];
}

export function formatBytes(n: number | null): string {
  if (n == null) return "—";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export async function listDocApps(): Promise<AppCard[]> {
  const data = await getJson<{ apps: AppCard[] }>("/api/docs");
  return data.apps;
}

export async function getOverview(appId: string): Promise<Overview> {
  const data = await getJson<{ data: Overview }>(`/api/docs/${appId}/overview`);
  return data.data;
}

export async function getEntry(
  appId: string,
  docType: string,
  name: string,
): Promise<DocEntry> {
  const data = await getJson<{ data: DocEntry }>(
    `/api/docs/${appId}/${docType}/${name}`,
  );
  return data.data;
}
