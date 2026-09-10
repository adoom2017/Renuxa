import type { Subscription, Status } from './models';
import { colors } from './constants';

const configuredApiUrl = import.meta.env.VITE_API_URL?.replace(/\/$/, '');
export const apiUrl = configuredApiUrl
  ? `${configuredApiUrl.replace(/\/api$/, '')}/api`
  : undefined;

export async function apiRequest<T = Record<string, unknown>>(path: string, init: RequestInit = {}, token?: string | null): Promise<T> {
  if (!apiUrl) throw new Error('API is not configured');
  const headers = new Headers(init.headers);
  headers.set('content-type', 'application/json');
  if (token) headers.set('authorization', `Bearer ${token}`);
  const response = await fetch(`${apiUrl}${path}`, { ...init, headers });
  if (!response.ok) {
    const payload = await response.json().catch(() => null) as {error?:{message?:string}} | null;
    throw Object.assign(new Error(payload?.error?.message ?? '请求失败'), {status:response.status});
  }
  return (response.status === 204 ? null : await response.json()) as T;
}

export function remoteSubscription(row: Record<string, unknown>): Subscription {
  return {
    id: String(row.id), name: String(row.name), plan: String(row.plan_name ?? '标准方案'),
    amount: Number(row.amount), currency: String(row.currency), cadence: String(row.cadence_unit), cadenceInterval: Number(row.cadence_interval), anchorDay: Number(row.anchor_day),
    startDate: row.start_date ? String(row.start_date) : undefined, nextDate: String(row.next_billing_date), category: String(row.category), status: String(row.status) as Status,
    color: colors[String(row.name).length % colors.length], iconUrl: row.icon_url ? String(row.icon_url) : undefined,
    reminderOffsets: Array.isArray(row.reminder_offsets) ? row.reminder_offsets.map(Number) : undefined,
  };
}


export async function allBills(token: string) {
  const bills: Record<string, unknown>[] = [];
  for (;;) {
    const page = await apiRequest<Record<string, unknown>[]>(`/bills?offset=${bills.length}`, {}, token);
    bills.push(...page);
    if (page.length < 200) return bills;
  }
}
