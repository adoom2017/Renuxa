'use client';

import {
  Bell, CalendarDays, Check, ChevronLeft, ChevronRight, CircleDollarSign, CreditCard, Globe2,
  LayoutDashboard, Mail, Menu, Pause, Play, Plus, ReceiptText, Send,
  RefreshCw, Search, Settings, ShieldCheck, SlidersHorizontal, Trash2,
  WalletCards, X,
} from 'lucide-react';
import { FormEvent, useEffect, useRef, useState } from 'react';

type View = 'dashboard' | 'subscriptions' | 'bills' | 'notifications' | 'settings';
type Status = 'active' | 'paused' | 'cancelled';
type BillStatus = 'estimated' | 'paid' | 'skipped' | 'refunded';
type Locale = 'zh-CN' | 'en';

type Subscription = {
  id: string; name: string; plan: string; amount: number; currency: string; cadence: string;
  nextDate: string; category: string; status: Status; color: string; iconUrl?: string; reminderOffsets?: number[];
};

type Bill = { id: string; subscription: string; date: string; amount: number; currency: string; status: BillStatus };
type Notice = { id: string; title: string; body: string; date: string; read: boolean; kind: 'renewal' | 'bill' | 'system' };
type NotificationSettings = {
  telegram_enabled: boolean; telegram_bot_token_configured: boolean; telegram_chat_id: string;
  email_enabled: boolean; smtp_host: string; smtp_port: number; smtp_tls: boolean;
  smtp_from: string; smtp_username: string; smtp_password_configured: boolean;
};

const seedSubscriptions: Subscription[] = [
  { id: 'sub-figma', name: 'Figma', plan: 'Professional', amount: 15, currency: 'USD', cadence: 'monthly', nextDate: '2026-09-03', category: '工作效率', status: 'active', color: '#242424' },
  { id: 'sub-icloud', name: 'iCloud+', plan: '2 TB', amount: 68, currency: 'CNY', cadence: 'monthly', nextDate: '2026-09-06', category: '云服务', status: 'active', color: '#2688e6' },
  { id: 'sub-netflix', name: 'Netflix', plan: '标准套餐', amount: 73, currency: 'HKD', cadence: 'monthly', nextDate: '2026-09-12', category: '影音娱乐', status: 'active', color: '#e5232b' },
  { id: 'sub-notion', name: 'Notion', plan: 'Plus', amount: 10, currency: 'USD', cadence: 'monthly', nextDate: '2026-09-18', category: '工作效率', status: 'active', color: '#353535' },
  { id: 'sub-applemusic', name: 'Apple Music', plan: '个人', amount: 11, currency: 'CNY', cadence: 'monthly', nextDate: '2026-09-22', category: '影音娱乐', status: 'active', color: '#ef4962' },
  { id: 'sub-dropbox', name: 'Dropbox', plan: 'Plus', amount: 119.88, currency: 'USD', cadence: 'yearly', nextDate: '2027-01-14', category: '云服务', status: 'paused', color: '#1877f2' },
];

const seedBills: Bill[] = [
  { id: 'bill-1', subscription: 'iCloud+', date: '2026-08-06', amount: 68, currency: 'CNY', status: 'paid' },
  { id: 'bill-2', subscription: 'Figma', date: '2026-08-03', amount: 15, currency: 'USD', status: 'paid' },
  { id: 'bill-3', subscription: 'Netflix', date: '2026-08-12', amount: 73, currency: 'HKD', status: 'paid' },
  { id: 'bill-4', subscription: 'Notion', date: '2026-09-18', amount: 10, currency: 'USD', status: 'estimated' },
  { id: 'bill-5', subscription: 'Apple Music', date: '2026-09-22', amount: 11, currency: 'CNY', status: 'estimated' },
];

const seedNotices: Notice[] = [
  { id: 'n-1', title: 'Figma 将在 2 天后续费', body: '预计扣款 US$15.00，到期提醒已安排。', date: '今天 09:00', read: false, kind: 'renewal' },
  { id: 'n-2', title: 'iCloud+ 账单已确认', body: '8 月账单 CN¥68.00 已计入支出统计。', date: '8月6日', read: false, kind: 'bill' },
  { id: 'n-3', title: '每日汇率已更新', body: '当前汇率数据日期为 2026 年 8 月 31 日。', date: '昨天', read: false, kind: 'system' },
];

const rates: Record<string, number> = { CNY: 1, USD: 7.12, HKD: 0.91, EUR: 8.31, JPY: 0.048, GBP: 9.55 };
const currencySymbols: Record<string, string> = { CNY: '¥', USD: 'US$', HKD: 'HK$', EUR: '€', JPY: 'JP¥', GBP: '£' };
const colors = ['#1f6b50', '#375aa7', '#b34d45', '#75603c', '#5a4f94', '#277a7a'];
const configuredApiUrl = import.meta.env.VITE_API_URL?.replace(/\/$/, '');
const apiUrl = configuredApiUrl
  ? `${configuredApiUrl.replace(/\/api$/, '')}/api`
  : undefined;

async function apiRequest(path: string, init: RequestInit = {}, token?: string | null) {
  if (!apiUrl) throw new Error('API is not configured');
  const headers = new Headers(init.headers);
  headers.set('content-type', 'application/json');
  if (token) headers.set('authorization', `Bearer ${token}`);
  const response = await fetch(`${apiUrl}${path}`, { ...init, headers });
  if (!response.ok) {
    const payload = await response.json().catch(() => null);
    throw new Error(payload?.error?.message ?? '请求失败');
  }
  return response.status === 204 ? null : response.json();
}

function remoteSubscription(row: Record<string, unknown>): Subscription {
  return {
    id: String(row.id), name: String(row.name), plan: String(row.plan_name ?? '标准方案'),
    amount: Number(row.amount), currency: String(row.currency), cadence: String(row.cadence_unit) === 'year' ? 'yearly' : String(row.cadence_unit) === 'quarter' ? 'quarterly' : 'monthly',
    nextDate: String(row.next_billing_date), category: String(row.category), status: String(row.status) as Status,
    color: colors[String(row.name).length % colors.length], iconUrl: row.icon_url ? String(row.icon_url) : undefined,
  };
}

const copy = {
  'zh-CN': {
    nav: ['工作台', '我的订阅', '账单记录', '通知中心', '设置'], greeting: '让每一次续费，都心中有数', add: '添加订阅',
    expected: '本月预计', yearly: '年度预计', active: '活跃订阅', trend: '支出趋势', months: '过去 6 个月', upcoming: '即将续费', next14: '未来 14 天', all: '查看全部',
    subscriptions: '我的订阅', subDesc: '集中查看价格、周期和下一次续费日期', bills: '账单记录', billsDesc: '核对预计扣款和真实支出', notifications: '通知中心', noticesDesc: '续费、账单与系统消息',
    settings: '偏好设置', settingsDesc: '管理货币、语言和提醒规则', search: '搜索订阅', markAll: '全部已读',
  },
  en: {
    nav: ['Dashboard', 'Subscriptions', 'Bills', 'Notifications', 'Settings'], greeting: 'Every renewal, accounted for', add: 'Add subscription',
    expected: 'Expected this month', yearly: 'Yearly forecast', active: 'Active subscriptions', trend: 'Spending trend', months: 'Past 6 months', upcoming: 'Upcoming renewals', next14: 'Next 14 days', all: 'View all',
    subscriptions: 'Subscriptions', subDesc: 'Review pricing, cadence, and upcoming renewals', bills: 'Bills', billsDesc: 'Reconcile forecasts with actual charges', notifications: 'Notifications', noticesDesc: 'Renewals, bills, and system updates',
    settings: 'Preferences', settingsDesc: 'Manage currency, language, and reminder rules', search: 'Search subscriptions', markAll: 'Mark all read',
  },
};

function useStoredState<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(initial);
  const [ready, setReady] = useState(false);
  const hydrated = useRef(false);
  useEffect(() => {
    const saved = localStorage.getItem(key);
    queueMicrotask(() => {
      if (saved) { try { setValue(JSON.parse(saved)); } catch {} }
      hydrated.current = true;
      setReady(true);
    });
  }, [key]);
  useEffect(() => { if (hydrated.current) localStorage.setItem(key, JSON.stringify(value)); }, [key, value]);
  return [value, setValue, ready] as const;
}

function money(amount: number, currency: string) {
  return `${currencySymbols[currency] ?? `${currency} `}${amount.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

function iconSource(iconUrl?: string) {
  if (!iconUrl || !apiUrl) return iconUrl;
  try {
    const url = new URL(iconUrl);
    if (url.protocol === 'https:' && (url.hostname === 'mzstatic.com' || url.hostname.endsWith('.mzstatic.com'))) {
      return `${apiUrl}/icons/image?url=${encodeURIComponent(iconUrl)}`;
    }
  } catch {}
  return iconUrl;
}

function ServiceIconImage({ source, fallback }: { source: string; fallback: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) return fallback;
  return <>
    {/* eslint-disable-next-line @next/next/no-img-element */}
    <img src={source} alt="" width={56} height={56} onError={() => setFailed(true)} />
  </>;
}

function addBillingMonths(date: Date, months: number) {
  const day = date.getDate();
  const result = new Date(date.getFullYear(), date.getMonth() + months, 1);
  const lastDay = new Date(result.getFullYear(), result.getMonth() + 1, 0).getDate();
  result.setDate(Math.min(day, lastDay));
  return result;
}

function scheduledSpend(subscriptions: Subscription[], start: Date, end: Date) {
  return subscriptions.reduce((total, sub) => {
    const interval = sub.cadence === 'yearly' ? 12 : sub.cadence === 'quarterly' ? 3 : 1;
    let billingDate = new Date(`${sub.nextDate}T00:00:00`);
    if (Number.isNaN(billingDate.getTime())) return total;
    while (addBillingMonths(billingDate, -interval) >= start) {
      billingDate = addBillingMonths(billingDate, -interval);
    }
    while (billingDate < start) billingDate = addBillingMonths(billingDate, interval);
    while (billingDate < end) {
      total += sub.amount * (rates[sub.currency] ?? 0);
      billingDate = addBillingMonths(billingDate, interval);
    }
    return total;
  }, 0);
}

function ServiceIcon({ item, size = 'normal' }: { item: Pick<Subscription, 'name' | 'color' | 'iconUrl'>; size?: 'normal' | 'large' }) {
  const source = iconSource(item.iconUrl);
  const fallback = item.name.slice(0, 1).toUpperCase();
  return <span className={`service-icon ${size === 'large' ? 'large' : ''}`} style={{ background: item.color }}>
    {source ? <ServiceIconImage key={source} source={source} fallback={fallback} /> : fallback}
  </span>;
}

export default function RenuxaApp() {
  const [view, setView] = useState<View>('dashboard');
  const [subscriptions, setSubscriptions] = useStoredState('renuxa.subscriptions', seedSubscriptions);
  const [bills, setBills] = useStoredState('renuxa.bills', seedBills);
  const [notices, setNotices] = useStoredState('renuxa.notices', seedNotices);
  const [locale, setLocale] = useStoredState<Locale>('renuxa.locale', 'zh-CN');
  const [baseCurrency, setBaseCurrency] = useStoredState('renuxa.currency', 'CNY');
  const [token, setToken, tokenReady] = useStoredState<string | null>('renuxa.token', null);
  const [userEmail, setUserEmail] = useStoredState('renuxa.user-email', '');
  const [modalOpen, setModalOpen] = useState(false);
  const [mobileNav, setMobileNav] = useState(false);
  const t = copy[locale];
  const unread = notices.filter((notice) => !notice.read).length;

  useEffect(() => {
    if (!apiUrl || !token) return;
    Promise.all([
      apiRequest('/subscriptions', {}, token), apiRequest('/bills', {}, token), apiRequest('/notifications', {}, token),
    ]).then(([remoteSubs, remoteBills, remoteNotices]) => {
      setSubscriptions((remoteSubs as Record<string, unknown>[]).map(remoteSubscription));
      setBills((remoteBills as Record<string, unknown>[]).map((row) => ({ id:String(row.id), subscription:String(row.subscription_name), date:String(row.due_date), amount:Number(row.amount), currency:String(row.currency), status:String(row.status) as BillStatus })));
      setNotices((remoteNotices as Record<string, unknown>[]).map((row) => ({ id:String(row.id), title:String(row.title), body:String(row.body), date:new Date(String(row.scheduled_for)).toLocaleString(), read:Boolean(row.read_at), kind:String(row.kind)==='renewal'?'renewal':'system' })));
    }).catch(() => setToken(null));
  }, [token, setBills, setNotices, setSubscriptions, setToken]);

  const nav: { id: View; icon: typeof LayoutDashboard }[] = [
    { id: 'dashboard', icon: LayoutDashboard }, { id: 'subscriptions', icon: CreditCard },
    { id: 'bills', icon: ReceiptText }, { id: 'notifications', icon: Bell }, { id: 'settings', icon: Settings },
  ];

  const changeView = (next: View) => { setView(next); setMobileNav(false); };
  const addSubscription = (sub: Subscription) => {
    setSubscriptions((current) => [sub, ...current]);
    setNotices((current) => [{ id: crypto.randomUUID(), title: `${sub.name} 已添加`, body: `${money(sub.amount, sub.currency)} · ${sub.nextDate}`, date: '刚刚', read: false, kind: 'system' }, ...current]);
    setModalOpen(false); setView('subscriptions');
    if (token) void apiRequest('/subscriptions', { method:'POST', body:JSON.stringify({ name:sub.name, plan_name:sub.plan, amount:String(sub.amount), currency:sub.currency, cadence_unit:sub.cadence==='yearly'?'year':sub.cadence==='quarterly'?'quarter':'month', cadence_interval:1, next_billing_date:sub.nextDate, category:sub.category, icon_url:sub.iconUrl, reminder_offsets:sub.reminderOffsets ?? [7,3,1] }) }, token)
      .then((row) => setSubscriptions((current) => current.map((item) => item.id === sub.id ? remoteSubscription(row) : item))).catch(() => undefined);
  };
  const updateStatus = (id: string, status: Status) => { setSubscriptions((current) => current.map((sub) => sub.id === id ? { ...sub, status } : sub)); if(token) void apiRequest(`/subscriptions/${id}`, {method:'PATCH',body:JSON.stringify({status})}, token).catch(()=>undefined); };
  const removeSubscription = (id: string) => { setSubscriptions((current) => current.filter((sub) => sub.id !== id)); if(token) void apiRequest(`/subscriptions/${id}`, {method:'DELETE'}, token).catch(()=>undefined); };
  const updateBill = (id:string,status:BillStatus) => { setBills((all) => all.map((bill) => bill.id === id ? { ...bill, status } : bill)); if(token) void apiRequest(`/bills/${id}`, {method:'PATCH',body:JSON.stringify({status})}, token).catch(()=>undefined); };
  const readNotice = (id:string) => { setNotices((all) => all.map((n) => n.id === id ? { ...n, read: true } : n)); if(token) void apiRequest(`/notifications/${id}/read`, {method:'POST'}, token).catch(()=>undefined); };
  const readAllNotices = () => {
    const unreadIds = notices.filter((notice) => !notice.read).map((notice) => notice.id);
    setNotices((all) => all.map((notice) => ({ ...notice, read: true })));
    if (token) void Promise.all(unreadIds.map((id) => apiRequest(`/notifications/${id}/read`, { method:'POST' }, token))).catch(() => undefined);
  };

  if (apiUrl && !tokenReady) return <div className="app-loading" aria-hidden="true" />;
  if (apiUrl && !token) return <AuthScreen onAuthenticated={(session) => { setToken(session.access_token); setUserEmail(session.email); }} />;

  return (
    <main className="app-shell">
      <aside className={`sidebar ${mobileNav ? 'open' : ''}`}>
        <button className="brand" onClick={() => changeView('dashboard')} aria-label="续序首页">
          <span className="brand-mark"><img src="/renuxa-logo.svg" alt="" /></span><span><strong>续序</strong><small>Renuxa</small></span>
        </button>
        <nav className="nav" aria-label="主导航">
          {nav.map(({ id, icon: Icon }, index) => <button key={id} className={view === id ? 'active' : ''} onClick={() => changeView(id)}>
            <Icon size={18} strokeWidth={1.8} /><span>{t.nav[index]}</span>{id === 'notifications' && unread > 0 && <i>{unread}</i>}
          </button>)}
        </nav>
        <div className="sidebar-profile"><span>沈</span><div><strong>{locale === 'zh-CN' ? '下午好' : 'Good afternoon'}</strong><small>shendongchun</small></div></div>
      </aside>

      <section className="content">
        <div className="mobile-topbar"><button onClick={() => setMobileNav(!mobileNav)} aria-label="打开菜单"><Menu /></button><strong><img src="/renuxa-logo.svg" alt="Renuxa" />续序</strong><button onClick={() => setModalOpen(true)} aria-label={t.add}><Plus /></button></div>
        {view === 'dashboard' && <Dashboard subscriptions={subscriptions} bills={bills} currency={baseCurrency} t={t} onAdd={() => setModalOpen(true)} onView={changeView} />}
        {view === 'subscriptions' && <SubscriptionsView subscriptions={subscriptions} t={t} onAdd={() => setModalOpen(true)} onStatus={updateStatus} onRemove={removeSubscription} />}
        {view === 'bills' && <BillsView bills={bills} t={t} onUpdate={updateBill} />}
        {view === 'notifications' && <NotificationsView notices={notices} t={t} onRead={readNotice} onReadAll={readAllNotices} />}
        {view === 'settings' && <SettingsView locale={locale} setLocale={setLocale} currency={baseCurrency} setCurrency={setBaseCurrency} t={t} token={token} userEmail={userEmail} onLogout={() => { setToken(null); setUserEmail(''); }} />}
      </section>
      {modalOpen && <AddSubscriptionModal locale={locale} onClose={() => setModalOpen(false)} onSave={addSubscription} />}
    </main>
  );
}

function AuthScreen({ onAuthenticated }: { onAuthenticated:(session:{ access_token:string; email:string })=>void }) {
  const [mode,setMode]=useState<'login'|'register'>('login'); const [email,setEmail]=useState(''); const [password,setPassword]=useState(''); const [error,setError]=useState(''); const [busy,setBusy]=useState(false);
  const submit=async(event:FormEvent)=>{event.preventDefault();setBusy(true);setError('');try{const result=await apiRequest(`/auth/${mode}`,{method:'POST',body:JSON.stringify({email,password})});onAuthenticated(result);}catch(reason){setError(reason instanceof Error?reason.message:'请求失败');}finally{setBusy(false);}};
  return <main className="auth-shell"><section className="auth-brand"><span className="brand-mark"><img src="/renuxa-logo.svg" alt="" /></span><div><strong>续序</strong><small>Renuxa</small></div><h1>让每一次续费，<br/>都心中有数</h1><p>订阅、账单、汇率和提醒，在同一处保持有序。</p></section><section className="auth-form-wrap"><form className="auth-form" onSubmit={submit}><p>RENuxa ACCOUNT</p><h2>{mode==='login'?'登录续序':'创建账户'}</h2><span>{mode==='login'?'继续管理你的所有订阅':'开始建立清晰的订阅账本'}</span><label className="field"><b>邮箱</b><input type="email" required value={email} onChange={(e)=>setEmail(e.target.value)} placeholder="name@example.com"/></label><label className="field"><b>密码</b><input type="password" required minLength={10} value={password} onChange={(e)=>setPassword(e.target.value)} placeholder="至少 10 位"/></label>{error&&<div className="form-error">{error}</div>}<button className="primary auth-submit" disabled={busy}>{busy?<RefreshCw className="spin"/>:mode==='login'?'登录':'注册'}</button><button className="auth-switch" type="button" onClick={()=>{setMode(mode==='login'?'register':'login');setError('')}}>{mode==='login'?'没有账户？创建一个':'已有账户？返回登录'}</button></form></section></main>;
}

function PageHeader({ eyebrow, title, description, action }: { eyebrow: string; title: string; description?: string; action?: React.ReactNode }) {
  return <header className="topbar"><div><p>{eyebrow}</p><h1>{title}</h1>{description && <span className="page-description">{description}</span>}</div>{action}</header>;
}

function Dashboard({ subscriptions, bills, currency, t, onAdd, onView }: { subscriptions: Subscription[]; bills: Bill[]; currency: string; t: typeof copy['zh-CN']; onAdd: () => void; onView: (view: View) => void }) {
  const active = subscriptions.filter((s) => s.status === 'active');
  const today = new Date();
  const monthStart = new Date(today.getFullYear(), today.getMonth(), 1);
  const nextMonthStart = new Date(today.getFullYear(), today.getMonth() + 1, 1);
  const yearStart = new Date(today.getFullYear(), 0, 1);
  const nextYearStart = new Date(today.getFullYear() + 1, 0, 1);
  const monthly = scheduledSpend(active, monthStart, nextMonthStart);
  const yearly = scheduledSpend(active, yearStart, nextYearStart);
  const displayTotal = currency === 'CNY' ? monthly : monthly / (rates[currency] ?? 1);
  const displayYearly = currency === 'CNY' ? yearly : yearly / (rates[currency] ?? 1);
  const todayKey = today.toISOString().slice(0, 10);
  const upcomingLimit = new Date(today.getFullYear(), today.getMonth(), today.getDate() + 14).toISOString().slice(0, 10);
  const upcoming = active.filter((sub) => sub.nextDate >= todayKey && sub.nextDate <= upcomingLimit).sort((a, b) => a.nextDate.localeCompare(b.nextDate)).slice(0, 4);
  const paid = bills.filter((b) => b.status === 'paid').reduce((sum, b) => sum + b.amount * (rates[b.currency] ?? 0), 0);
  const paidDisplay = currency === 'CNY' ? paid : paid / (rates[currency] ?? 1);
  const trend = Array.from({ length: 6 }, (_, index) => {
    const month = new Date(today.getFullYear(), today.getMonth() - 5 + index, 1);
    const key = `${month.getFullYear()}-${String(month.getMonth() + 1).padStart(2, '0')}`;
    const amountCny = bills.filter((bill) => bill.status === 'paid' && bill.date.startsWith(key)).reduce((sum, bill) => sum + bill.amount * (rates[bill.currency] ?? 0), 0);
    return { key, label: `${month.getMonth() + 1}月`, amount: currency === 'CNY' ? amountCny : amountCny / (rates[currency] ?? 1) };
  });
  const maxTrend = Math.max(...trend.map((month) => month.amount), 0);
  return <>
    <PageHeader eyebrow={today.toLocaleDateString('zh-CN', { year: 'numeric', month: 'long', day: 'numeric', weekday: 'long' })} title={t.greeting} action={<button className="primary" onClick={onAdd}><Plus size={17} />{t.add}</button>} />
    <section className="metrics" aria-label="订阅概览">
      <article className="metric primary-metric"><div className="metric-label"><span>{t.expected}</span></div><strong>{money(displayTotal, currency)}</strong><p>按本月续费日期计算 · 已确认 {money(paidDisplay, currency)}</p></article>
      <article className="metric"><span>{t.yearly}</span><strong>{money(displayYearly, currency)}</strong><p>按当前年度实际续费日期预计</p></article>
      <article className="metric"><span>{t.active}</span><strong>{active.length}</strong><p>{new Set(active.map((s) => s.currency)).size} 种货币 · {new Set(active.map((s) => s.category)).size} 个分类</p></article>
    </section>
    <div className="dashboard-grid">
      <section className="panel spending-panel"><div className="panel-heading"><div><p>{t.trend}</p><h2>{t.months}</h2></div><span className="quiet-select">{currency}</span></div>{maxTrend > 0 ? <div className="chart" aria-label="过去六个月支出柱状图">{trend.map((month)=><div className="bar-column" key={month.key}><div className="bar-track"><span style={{height:`${Math.max(8, month.amount / maxTrend * 100)}%`}} title={money(month.amount, currency)} /></div><small>{month.label}</small></div>)}</div> : <div className="chart-empty"><ReceiptText/><strong>暂无支出记录</strong><span>确认账单后，这里会显示最近六个月的支出趋势。</span></div>}</section>
      <section className="panel upcoming-panel"><div className="panel-heading"><div><p>{t.upcoming}</p><h2>{t.next14}</h2></div><button className="text-button" onClick={() => onView('subscriptions')}>{t.all}<ChevronRight size={14}/></button></div><div className="upcoming-list">{upcoming.map((item)=><article className="subscription-row" key={item.id}><ServiceIcon item={item}/><div className="service-copy"><strong>{item.name}</strong><small>{item.plan} · {item.nextDate.slice(5).replace('-', '月')}日</small></div><strong className="price">{money(item.amount,item.currency)}</strong></article>)}{upcoming.length === 0 && <div className="compact-empty"><CalendarDays/><span>未来 14 天暂无续费</span></div>}</div></section>
    </div>
    <SpendingCalendar subscriptions={active} currency={currency} />
    <section className="notice-band"><span className="notice-icon"><Bell size={17}/></span><div><strong>{upcoming[0] ? `${upcoming[0].name} 将在近期续费` : '暂无近期续费'}</strong><p>{upcoming[0] ? `预计扣款 ${money(upcoming[0].amount,upcoming[0].currency)}，邮件提醒已安排。` : '添加订阅后，续序会在到期前提醒你。'}</p></div><button onClick={() => onView('notifications')}>{t.all}</button></section>
  </>;
}

function dateKey(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}

function SpendingCalendar({ subscriptions, currency }: { subscriptions: Subscription[]; currency: string }) {
  const [month, setMonth] = useState(() => new Date(new Date().getFullYear(), new Date().getMonth(), 1));
  const monthEnd = new Date(month.getFullYear(), month.getMonth() + 1, 1);
  const firstWeekday = month.getDay();
  const days = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate();
  const entries = new Map<string, Subscription[]>();
  subscriptions.forEach((sub) => {
    const interval = sub.cadence === 'yearly' ? 12 : sub.cadence === 'quarterly' ? 3 : 1;
    let billingDate = new Date(`${sub.nextDate}T00:00:00`);
    if (Number.isNaN(billingDate.getTime())) return;
    while (addBillingMonths(billingDate, -interval) >= month) billingDate = addBillingMonths(billingDate, -interval);
    while (billingDate < month) billingDate = addBillingMonths(billingDate, interval);
    while (billingDate < monthEnd) {
      const key = dateKey(billingDate);
      entries.set(key, [...(entries.get(key) ?? []), sub]);
      billingDate = addBillingMonths(billingDate, interval);
    }
  });
  const todayKey = dateKey(new Date());
  return <section className="panel calendar-panel"><div className="panel-heading"><div><p>SPENDING CALENDAR</p><h2>{month.toLocaleDateString('zh-CN', { year: 'numeric', month: 'long' })}</h2></div><div className="calendar-nav"><button title="上个月" aria-label="上个月" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}><ChevronLeft size={16}/></button><button title="下个月" aria-label="下个月" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}><ChevronRight size={16}/></button></div></div><div className="calendar-grid">{['日','一','二','三','四','五','六'].map((day) => <span className="calendar-weekday" key={day}>{day}</span>)}{Array.from({ length: firstWeekday }, (_, index) => <span className="calendar-empty" key={`empty-${index}`} />)}{Array.from({ length: days }, (_, index) => { const day = index + 1; const date = new Date(month.getFullYear(), month.getMonth(), day); const items = entries.get(dateKey(date)) ?? []; return <div className={`calendar-day ${dateKey(date) === todayKey ? 'today' : ''}`} key={day}><b>{day}</b>{items.map((item) => { const converted = item.amount * (rates[item.currency] ?? 0) / (rates[currency] ?? 1); return <span className="calendar-entry" title={`${item.name} · ${money(item.amount, item.currency)}`} key={`${item.id}-${day}`}><i style={{ background: item.color }} />{item.name}<em>{money(converted, currency)}</em></span>; })}</div>; })}</div></section>;
}

function SubscriptionsView({ subscriptions, t, onAdd, onStatus, onRemove }: { subscriptions: Subscription[]; t: typeof copy['zh-CN']; onAdd: () => void; onStatus: (id:string,status:Status)=>void; onRemove:(id:string)=>void }) {
  const [query, setQuery] = useState(''); const [filter, setFilter] = useState<'all'|Status>('all');
  const shown = subscriptions.filter((s) => (filter === 'all' || s.status === filter) && `${s.name} ${s.plan} ${s.category}`.toLowerCase().includes(query.toLowerCase()));
  return <>
    <PageHeader eyebrow="SUBSCRIPTIONS" title={t.subscriptions} description={t.subDesc} action={<button className="primary" onClick={onAdd}><Plus size={17}/>{t.add}</button>} />
    <div className="toolbar"><label className="search-field"><Search size={16}/><input value={query} onChange={(e)=>setQuery(e.target.value)} placeholder={t.search}/></label><label className="filter-select"><SlidersHorizontal size={15}/><select value={filter} onChange={(e)=>setFilter(e.target.value as typeof filter)}><option value="all">全部状态</option><option value="active">使用中</option><option value="paused">已暂停</option><option value="cancelled">已取消</option></select></label></div>
    <section className="data-panel subscriptions-table"><div className="table-head"><span>订阅服务</span><span>分类</span><span>周期</span><span>下次续费</span><span>金额</span><span>状态</span><span /></div>{shown.map((sub)=><article className="table-row" key={sub.id}><div className="service-cell"><ServiceIcon item={sub}/><span><strong>{sub.name}</strong><small>{sub.plan}</small></span></div><span>{sub.category}</span><span>{sub.cadence === 'yearly' ? '每年' : sub.cadence === 'quarterly' ? '每季度' : '每月'}</span><span>{sub.nextDate}</span><strong>{money(sub.amount,sub.currency)}</strong><StatusBadge status={sub.status}/><div className="row-actions">{sub.status === 'active' ? <button title="暂停" onClick={()=>onStatus(sub.id,'paused')}><Pause size={15}/></button> : <button title="恢复" onClick={()=>onStatus(sub.id,'active')}><Play size={15}/></button>}<button title="归档" onClick={()=>onRemove(sub.id)}><Trash2 size={15}/></button></div></article>)}</section>
    {shown.length === 0 && <div className="empty-state"><Search/><strong>没有找到订阅</strong><span>调整搜索或筛选条件后再试。</span></div>}
  </>;
}

function StatusBadge({ status }: { status: Status }) { const labels={active:'使用中',paused:'已暂停',cancelled:'已取消'}; return <span className={`status ${status}`}>{labels[status]}</span>; }
function BillBadge({ status }: { status: BillStatus }) { const labels={estimated:'待确认',paid:'已支付',skipped:'未扣款',refunded:'已退款'}; return <span className={`status ${status}`}>{labels[status]}</span>; }

function BillsView({ bills, t, onUpdate }: { bills: Bill[]; t: typeof copy['zh-CN']; onUpdate:(id:string,status:BillStatus)=>void }) {
  const [filter,setFilter]=useState<'all'|BillStatus>('all'); const shown=bills.filter((b)=>filter==='all'||b.status===filter);
  const total=bills.filter((b)=>b.status==='paid').reduce((sum,b)=>sum+b.amount*(rates[b.currency]??0),0);
  return <><PageHeader eyebrow="BILLING" title={t.bills} description={t.billsDesc}/><div className="bill-summary"><div><CircleDollarSign/><span>今年已支付<small>按 CNY 折算</small></span><strong>{money(total,'CNY')}</strong></div><div><CalendarDays/><span>待确认账单<small>未来 30 天</small></span><strong>{bills.filter((b)=>b.status==='estimated').length}</strong></div><div><WalletCards/><span>记录总数<small>全部币种</small></span><strong>{bills.length}</strong></div></div><div className="segmented">{(['all','estimated','paid','skipped','refunded'] as const).map((value)=><button className={filter===value?'active':''} key={value} onClick={()=>setFilter(value)}>{value==='all'?'全部':{estimated:'待确认',paid:'已支付',skipped:'未扣款',refunded:'已退款'}[value]}</button>)}</div><section className="data-panel bills-table"><div className="table-head"><span>订阅</span><span>账单日</span><span>原币金额</span><span>折合 CNY</span><span>状态</span><span /></div>{shown.map((bill)=><article className="table-row" key={bill.id}><strong>{bill.subscription}</strong><span>{bill.date}</span><strong>{money(bill.amount,bill.currency)}</strong><span>{money(bill.amount*(rates[bill.currency]??0),'CNY')}</span><BillBadge status={bill.status}/><div className="bill-action">{bill.status==='estimated'&&<button onClick={()=>onUpdate(bill.id,'paid')}><Check size={14}/>确认支付</button>}</div></article>)}</section>{shown.length === 0 && <div className="empty-state"><ReceiptText/><strong>暂无账单记录</strong><span>{filter === 'all' ? '订阅到期后，账单会自动记录在这里。' : '当前筛选条件下没有账单。'}</span></div>}</>;
}

function NotificationsView({ notices, t, onRead, onReadAll }: { notices: Notice[]; t: typeof copy['zh-CN']; onRead:(id:string)=>void; onReadAll:()=>void }) {
  return <><PageHeader eyebrow="INBOX" title={t.notifications} description={t.noticesDesc} action={<button className="secondary" onClick={onReadAll} disabled={!notices.some((notice) => !notice.read)}><Check size={16}/>{t.markAll}</button>}/>{notices.length > 0 ? <section className="notification-list">{notices.map((notice)=><button key={notice.id} className={`notification-item ${notice.read?'read':''}`} onClick={()=>onRead(notice.id)}><span className={`notification-kind ${notice.kind}`}>{notice.kind==='renewal'?<RefreshCw/>:notice.kind==='bill'?<ReceiptText/>:<Globe2/>}</span><span className="notification-copy"><strong>{notice.title}</strong><small>{notice.body}</small></span><time>{notice.date}</time>{!notice.read&&<i/>}</button>)}</section> : <div className="empty-state page-empty"><Bell/><strong>暂无通知</strong><span>续费提醒和账单消息会显示在这里。</span></div>}</>;
}

const defaultNotificationSettings: NotificationSettings = {
  telegram_enabled: false, telegram_bot_token_configured: false, telegram_chat_id: '',
  email_enabled: false, smtp_host: '', smtp_port: 587, smtp_tls: true,
  smtp_from: '', smtp_username: '', smtp_password_configured: false,
};

function SettingsView({ locale, setLocale, currency, setCurrency, t, token, userEmail, onLogout }: { locale:Locale; setLocale:(v:Locale)=>void; currency:string; setCurrency:(v:string)=>void; t:typeof copy['zh-CN']; token:string|null; userEmail:string; onLogout:()=>void }) {
  const [tab,setTab]=useState<'general'|'notifications'|'security'>('general');
  const [timezone,setTimezone]=useStoredState('renuxa.timezone','Asia/Shanghai');
  const [reminders,setReminders]=useStoredState<number[]>('renuxa.reminders',[7,3,1]);
  const [notificationSettings,setNotificationSettings]=useState(defaultNotificationSettings);
  const [telegramToken,setTelegramToken]=useState('');
  const [smtpPassword,setSmtpPassword]=useState('');
  const [saveState,setSaveState]=useState<'idle'|'saving'|'saved'|'error'>('idle');
  const [saveError,setSaveError]=useState('');

  useEffect(()=>{
    if (!token) return;
    apiRequest('/notification-settings',{},token)
      .then((value)=>setNotificationSettings(value as NotificationSettings))
      .catch(()=>setSaveError('通知设置加载失败'));
  },[token]);

  const updateNotificationSetting=<K extends keyof NotificationSettings,>(key:K,value:NotificationSettings[K])=>setNotificationSettings((current)=>({...current,[key]:value}));
  const saveNotificationSettings=async(event:FormEvent)=>{
    event.preventDefault();
    if (!token) return;
    setSaveState('saving'); setSaveError('');
    try {
      const saved=await apiRequest('/notification-settings',{method:'PUT',body:JSON.stringify({
        telegram_enabled:notificationSettings.telegram_enabled,
        telegram_bot_token:telegramToken||null,
        telegram_chat_id:notificationSettings.telegram_chat_id,
        email_enabled:notificationSettings.email_enabled,
        smtp_host:notificationSettings.smtp_host,
        smtp_port:notificationSettings.smtp_port,
        smtp_tls:notificationSettings.smtp_tls,
        smtp_from:notificationSettings.smtp_from,
        smtp_username:notificationSettings.smtp_username,
        smtp_password:smtpPassword||null,
      })},token) as NotificationSettings;
      setNotificationSettings(saved); setTelegramToken(''); setSmtpPassword(''); setSaveState('saved');
    } catch(reason) {
      setSaveState('error'); setSaveError(reason instanceof Error?reason.message:'保存失败');
    }
  };

  return <><PageHeader eyebrow="PREFERENCES" title={t.settings} description={t.settingsDesc}/><div className="settings-layout"><nav className="settings-nav" aria-label="设置分类"><button className={tab==='general'?'active':''} onClick={()=>setTab('general')}><Globe2/>通用</button><button className={tab==='notifications'?'active':''} onClick={()=>setTab('notifications')}><Bell/>通知</button><button className={tab==='security'?'active':''} onClick={()=>setTab('security')}><ShieldCheck/>账户与安全</button></nav><section className="settings-content">{tab==='general'&&<div className="settings-group"><h2>显示与地区</h2><SettingRow title="界面语言" description="更改界面中的文字语言"><select aria-label="界面语言" value={locale} onChange={(e)=>setLocale(e.target.value as Locale)}><option value="zh-CN">简体中文</option><option value="en">English</option></select></SettingRow><SettingRow title="基准货币" description="仪表盘和统计的默认折算货币"><select aria-label="基准货币" value={currency} onChange={(e)=>setCurrency(e.target.value)}>{Object.keys(rates).map((code)=><option key={code}>{code}</option>)}</select></SettingRow><SettingRow title="时区" description="用于界面中的日期和时间"><select aria-label="时区" value={timezone} onChange={(e)=>setTimezone(e.target.value)}><option>Asia/Shanghai</option><option>Asia/Hong_Kong</option><option>America/New_York</option><option>Europe/London</option></select></SettingRow></div>}{tab==='notifications'&&<form className="settings-group" onSubmit={saveNotificationSettings}><h2>通知渠道</h2><SettingRow title="应用内通知" description="续费提醒始终保留在通知中心"><span className="setting-value">始终启用</span></SettingRow><div className="notification-channel"><div className="channel-heading"><span className="channel-icon telegram"><Send/></span><div><strong>Telegram</strong><small>通过机器人发送续费提醒</small></div><button type="button" className={`toggle ${notificationSettings.telegram_enabled?'on':''}`} aria-label="启用 Telegram" aria-pressed={notificationSettings.telegram_enabled} onClick={()=>updateNotificationSetting('telegram_enabled',!notificationSettings.telegram_enabled)}><span/></button></div>{notificationSettings.telegram_enabled&&<div className="channel-fields"><label className="field"><span>Bot Token</span><input type="password" autoComplete="new-password" value={telegramToken} onChange={(e)=>setTelegramToken(e.target.value)} placeholder={notificationSettings.telegram_bot_token_configured?'已配置，留空保持不变':'从 BotFather 获取'}/></label><label className="field"><span>Chat ID</span><input value={notificationSettings.telegram_chat_id} onChange={(e)=>updateNotificationSetting('telegram_chat_id',e.target.value)} placeholder="例如：123456789"/></label></div>}</div><div className="notification-channel"><div className="channel-heading"><span className="channel-icon email"><Mail/></span><div><strong>邮件</strong><small>使用自有 SMTP 服务发送到登录邮箱</small></div><button type="button" className={`toggle ${notificationSettings.email_enabled?'on':''}`} aria-label="启用邮件" aria-pressed={notificationSettings.email_enabled} onClick={()=>updateNotificationSetting('email_enabled',!notificationSettings.email_enabled)}><span/></button></div>{notificationSettings.email_enabled&&<div className="channel-fields smtp-fields"><label className="field"><span>SMTP 主机</span><input value={notificationSettings.smtp_host} onChange={(e)=>updateNotificationSetting('smtp_host',e.target.value)} placeholder="smtp.example.com"/></label><label className="field"><span>端口</span><input type="number" min="1" max="65535" value={notificationSettings.smtp_port} onChange={(e)=>updateNotificationSetting('smtp_port',Number(e.target.value))}/></label><label className="field span-2"><span>发件人</span><input value={notificationSettings.smtp_from} onChange={(e)=>updateNotificationSetting('smtp_from',e.target.value)} placeholder="Renuxa <notifications@example.com>"/></label><label className="field"><span>用户名</span><input autoComplete="username" value={notificationSettings.smtp_username} onChange={(e)=>updateNotificationSetting('smtp_username',e.target.value)}/></label><label className="field"><span>密码</span><input type="password" autoComplete="new-password" value={smtpPassword} onChange={(e)=>setSmtpPassword(e.target.value)} placeholder={notificationSettings.smtp_password_configured?'已配置，留空保持不变':'SMTP 密码或密钥'}/></label><label className="tls-option"><input type="checkbox" checked={notificationSettings.smtp_tls} onChange={(e)=>updateNotificationSetting('smtp_tls',e.target.checked)}/><span>启用 TLS 加密连接</span></label></div>}</div><div className="reminder-row"><div><strong>提前提醒</strong><small>新订阅默认使用，可在单项中覆盖</small></div><div className="reminder-chips">{[14,7,3,1].map((day)=><button type="button" aria-pressed={reminders.includes(day)} key={day} className={reminders.includes(day)?'active':''} onClick={()=>setReminders(reminders.includes(day)?reminders.filter((v)=>v!==day):[...reminders,day].sort((a,b)=>b-a))}>{day} 天</button>)}</div></div><div className="settings-actions"><span className={saveState==='error'?'save-error':'save-status'}>{saveError||(saveState==='saved'?'设置已保存':'')}</span><button className="primary" disabled={!token||saveState==='saving'}>{saveState==='saving'?<RefreshCw className="spin"/>:<Check/>}保存设置</button></div></form>}{tab==='security'&&<div className="settings-group"><h2>账户与安全</h2><SettingRow title="当前账户" description={userEmail||'已连接 Renuxa 服务端'}><span className="setting-value">已登录</span></SettingRow><SettingRow title="退出登录" description="此设备上的订阅数据将在再次登录后同步"><button className="secondary" onClick={onLogout}>退出登录</button></SettingRow></div>}</section></div></>;
}

function SettingRow({ title, description, children }: { title:string; description:string; children:React.ReactNode }) { return <div className="setting-row"><div><strong>{title}</strong><small>{description}</small></div>{children}</div>; }

function AddSubscriptionModal({ locale, onClose, onSave }: { locale:Locale; onClose:()=>void; onSave:(sub:Subscription)=>void }) {
  const [name,setName]=useState(''); const [plan,setPlan]=useState(''); const [amount,setAmount]=useState(''); const [currency,setCurrency]=useState('CNY'); const [cadence,setCadence]=useState('monthly'); const [nextDate,setNextDate]=useState('2026-09-30'); const [category,setCategory]=useState('工作效率'); const [iconUrl,setIconUrl]=useState(''); const [iconBusy,setIconBusy]=useState(false); const [reminders,setReminders]=useState<number[]>(()=>{try{return JSON.parse(localStorage.getItem('renuxa.reminders')??'[7,3,1]')}catch{return [7,3,1]}});
  const searchIcon=async()=>{ if(!name.trim())return; setIconBusy(true); try { const results=await apiRequest(`/icons/search?q=${encodeURIComponent(name)}&country=cn`); setIconUrl(results?.[0]?.icon_url??''); } catch {} finally { setIconBusy(false); } };
  const submit=(event:FormEvent)=>{event.preventDefault(); const numeric=Number(amount); if(!name.trim()||!Number.isFinite(numeric)||numeric<0)return; onSave({id:crypto.randomUUID(),name:name.trim(),plan:plan.trim()||'标准方案',amount:numeric,currency,cadence,nextDate,category,status:'active',color:colors[name.length%colors.length],iconUrl:iconUrl||undefined,reminderOffsets:reminders});};
  return <div className="modal-backdrop" role="presentation" onMouseDown={(e)=>{if(e.currentTarget===e.target)onClose();}}><section className="modal" role="dialog" aria-modal="true" aria-labelledby="add-title"><header><div><p>NEW SUBSCRIPTION</p><h2 id="add-title">{locale==='zh-CN'?'添加订阅':'Add subscription'}</h2></div><button onClick={onClose} aria-label="关闭"><X/></button></header><form onSubmit={submit}><div className="icon-name-row"><ServiceIcon size="large" item={{name:name||'?',color:colors[name.length%colors.length],iconUrl:iconUrl||undefined}}/><label className="field grow"><span>订阅名称</span><div className="input-with-action"><input autoFocus required value={name} onChange={(e)=>setName(e.target.value)} onBlur={searchIcon} placeholder="例如：Spotify"/><button type="button" onClick={searchIcon} title="从 App Store 匹配图标">{iconBusy?<RefreshCw className="spin"/>:<Search/>}</button></div></label></div><div className="form-grid"><label className="field span-2"><span>方案名称</span><input value={plan} onChange={(e)=>setPlan(e.target.value)} placeholder="例如：个人高级版"/></label><label className="field"><span>金额</span><input required inputMode="decimal" value={amount} onChange={(e)=>setAmount(e.target.value)} placeholder="0.00"/></label><label className="field"><span>货币</span><select value={currency} onChange={(e)=>setCurrency(e.target.value)}>{Object.keys(rates).map((code)=><option key={code}>{code}</option>)}</select></label><label className="field"><span>扣费周期</span><select value={cadence} onChange={(e)=>setCadence(e.target.value)}><option value="monthly">每月</option><option value="quarterly">每季度</option><option value="yearly">每年</option></select></label><label className="field"><span>下次续费</span><input type="date" required value={nextDate} onChange={(e)=>setNextDate(e.target.value)}/></label><label className="field span-2"><span>分类</span><select value={category} onChange={(e)=>setCategory(e.target.value)}><option>工作效率</option><option>影音娱乐</option><option>云服务</option><option>学习教育</option><option>健康生活</option><option>其他</option></select></label></div><div className="reminder-config"><span><Bell size={15}/>提前提醒</span><div>{[14,7,3,1].map((day)=><button type="button" key={day} className={reminders.includes(day)?'active':''} onClick={()=>setReminders(reminders.includes(day)?reminders.filter((v)=>v!==day):[...reminders,day])}>{day} 天</button>)}</div></div><footer><button className="secondary" type="button" onClick={onClose}>取消</button><button className="primary" type="submit"><Plus size={16}/>添加订阅</button></footer></form></section></div>;
}
