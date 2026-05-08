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

export function formatBytes(n: number | null): string {
  if (n == null) return "—";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export async function listApps(): Promise<AppCard[]> {
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
