'use client';

import {
  Bell, CalendarDays, Check, ChevronLeft, ChevronRight, CreditCard, Globe2,
  LayoutDashboard, Menu, Pause, Pencil, Play, Plus, ReceiptText, Send,
  RefreshCw, Search, Settings, ShieldCheck, SlidersHorizontal, Trash2,
  X,
} from 'lucide-react';
import { FormEvent, useEffect, useRef, useState } from 'react';
import type { View, Status, BillStatus, Locale, Subscription, Bill, Notice, NotificationSettings } from './models';
import { seedSubscriptions, seedBills, seedNotices } from './demo-data';
import { rates, colors } from './constants';
import { apiUrl, apiRequest, remoteSubscription } from './api';
import { copy } from './copy';
import { useStoredState } from './use-stored-state';
import { money } from './format';
import { BillsView } from './bills-view';
import { billingDates, cadenceLabel, cadenceUnit } from './billing';

function AppImage({ src, alt, width, height, priority = false }: { src: string; alt: string; width: number; height: number; priority?: boolean }) {
  // This shared component must work in both the Next.js and standalone Vite builds.
  // eslint-disable-next-line @next/next/no-img-element
  return <img src={src} alt={alt} width={width} height={height} loading={priority ? 'eager' : 'lazy'} fetchPriority={priority ? 'high' : 'auto'} decoding="async" />;
}
function createLocalId() {
  // getRandomValues also works when a self-hosted instance is accessed over HTTP.
  return Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) => byte.toString(16).padStart(2, '0')).join('');
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


function scheduledSpend(subscriptions: Subscription[], start: Date, end: Date) {
  return subscriptions.reduce((total, sub) => {
    return total + billingDates(sub, start, end).length * sub.amount * (rates[sub.currency] ?? 0);
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
  const [editingSubscription, setEditingSubscription] = useState<Subscription | null>(null);
  const [mobileNav, setMobileNav] = useState(false);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [refreshError, setRefreshError] = useState('');
  const [loadedToken, setLoadedToken] = useState<string | null>(null);
  const t = copy[locale];
  const unread = notices.filter((notice) => !notice.read).length;

  useEffect(()=>{
    const refresh=()=>setRefreshVersion(v=>v+1);
    window.addEventListener('focus',refresh);
    return ()=>window.removeEventListener('focus',refresh);
  },[]);

  useEffect(() => {
    if (!apiUrl || !token) return;
    let cancelled = false;
    Promise.all([
      apiRequest<Record<string,unknown>[]>('/subscriptions', {}, token), apiRequest<Record<string,unknown>[]>('/bills', {}, token), apiRequest<Record<string,unknown>[]>('/notifications', {}, token),
    ]).then(([remoteSubs, remoteBills, remoteNotices]) => {
      if (cancelled) return;
      setLoadedToken(token);
      setRefreshError('');
      setSubscriptions((remoteSubs as Record<string, unknown>[]).map(remoteSubscription));
      setBills((remoteBills as Record<string, unknown>[]).map((row) => ({ id:String(row.id), subscriptionId:String(row.subscription_id), subscription:String(row.subscription_name), date:String(row.due_date), amount:Number(row.amount), currency:String(row.currency), status:String(row.status) as BillStatus })));
      setNotices((remoteNotices as Record<string, unknown>[]).map((row) => ({ id:String(row.id), title:String(row.title), body:String(row.body), date:new Date(String(row.scheduled_for)).toLocaleString(), read:Boolean(row.read_at), kind:String(row.kind)==='renewal'?'renewal':'system' })));
    }).catch((error) => { if (cancelled) return; if(error?.status===401) setToken(null); else setRefreshError(error instanceof Error?error.message:'刷新失败'); });
    return () => { cancelled = true; };
  }, [token, view, refreshVersion, setBills, setNotices, setSubscriptions, setToken]);

  const nav: { id: View; icon: typeof LayoutDashboard }[] = [
    { id: 'dashboard', icon: LayoutDashboard }, { id: 'subscriptions', icon: CreditCard },
    { id: 'bills', icon: ReceiptText }, { id: 'notifications', icon: Bell }, { id: 'settings', icon: Settings },
  ];

  const changeView = (next: View) => { setView(next); setMobileNav(false); };
  const addSubscription = async (sub: Subscription) => {
    const saved = token ? remoteSubscription(await apiRequest('/subscriptions', { method:'POST', body:JSON.stringify({ name:sub.name, plan_name:sub.plan, amount:String(sub.amount), currency:sub.currency, cadence_unit:cadenceUnit(sub.cadence), cadence_interval:sub.cadenceInterval??1, next_billing_date:sub.nextDate, category:sub.category, icon_url:sub.iconUrl, reminder_offsets:sub.reminderOffsets ?? [7,3,1] }) }, token)) : sub;
    setSubscriptions((current) => [saved, ...current]);
    setNotices((current) => [{ id: createLocalId(), title: `${saved.name} 已添加`, body: `${money(saved.amount, saved.currency)} · ${saved.nextDate}`, date: '刚刚', read: false, kind: 'system' }, ...current]);
    setModalOpen(false); setView('subscriptions');
  };
  const editSubscription = async (sub: Subscription) => {
    const saved = token ? remoteSubscription(await apiRequest(`/subscriptions/${sub.id}`, { method:'PATCH', body:JSON.stringify({ name:sub.name, plan_name:sub.plan, amount:String(sub.amount), currency:sub.currency, cadence_unit:cadenceUnit(sub.cadence), cadence_interval:sub.cadenceInterval??1, next_billing_date:sub.nextDate, category:sub.category, icon_url:sub.iconUrl, reminder_offsets:sub.reminderOffsets ?? [7,3,1] }) }, token)) : sub;
    setSubscriptions((current) => current.map((item) => item.id === saved.id ? saved : item));
    setEditingSubscription(null); setModalOpen(false); setRefreshVersion((value) => value + 1);
  };
  const updateStatus = (id: string, status: Status) => { setSubscriptions((current) => current.map((sub) => sub.id === id ? { ...sub, status } : sub)); if(token) void apiRequest(`/subscriptions/${id}`, {method:'PATCH',body:JSON.stringify({status})}, token).catch(()=>undefined); };
  const removeSubscription = (id: string) => { setSubscriptions((current) => current.filter((sub) => sub.id !== id)); if(token) void apiRequest(`/subscriptions/${id}`, {method:'DELETE'}, token).catch(()=>undefined); };
  const updateBill = async (id:string,status:BillStatus) => {
    if (apiUrl) await apiRequest(`/bills/${id}`, {method:'PATCH',body:JSON.stringify({status})}, token);
    setBills((all) => all.map((bill) => bill.id === id ? { ...bill, status } : bill));
    setRefreshVersion((version) => version + 1);
  };
  const readNotice = (id:string) => { setNotices((all) => all.map((n) => n.id === id ? { ...n, read: true } : n)); if(token) void apiRequest(`/notifications/${id}/read`, {method:'POST'}, token).catch(()=>undefined); };
  const readAllNotices = () => {
    const unreadIds = notices.filter((notice) => !notice.read).map((notice) => notice.id);
    setNotices((all) => all.map((notice) => ({ ...notice, read: true })));
    if (token) void Promise.all(unreadIds.map((id) => apiRequest(`/notifications/${id}/read`, { method:'POST' }, token))).catch(() => undefined);
  };

  if (apiUrl && !tokenReady) return <div className="app-loading" aria-hidden="true" />;
  if (apiUrl && !token) return <AuthScreen onAuthenticated={(session) => { setToken(session.access_token); setUserEmail(session.email); }} />;

  if (apiUrl && loadedToken !== token) return <main className="auth-shell"><section role="status"><p>{refreshError || '正在加载账户数据…'}</p>{refreshError && <button className="secondary" onClick={()=>setRefreshVersion(v=>v+1)}>重试</button>}</section></main>;

  return (
    <main className="app-shell">
      <aside className={`sidebar ${mobileNav ? 'open' : ''}`}>
        <button className="brand" onClick={() => changeView('dashboard')} aria-label="续序首页">
          <span className="brand-mark"><AppImage src="/renuxa-logo.svg" alt="" width={39} height={39} priority /></span><span><strong>续序</strong><small>Renuxa</small></span>
        </button>
        <nav className="nav" aria-label="主导航">
          {nav.map(({ id, icon: Icon }, index) => <button key={id} className={view === id ? 'active' : ''} onClick={() => changeView(id)}>
            <Icon size={18} strokeWidth={1.8} /><span>{t.nav[index]}</span>{id === 'notifications' && unread > 0 && <i>{unread}</i>}
          </button>)}
        </nav>
        <div className="sidebar-profile"><span>沈</span><div><strong>{locale === 'zh-CN' ? '下午好' : 'Good afternoon'}</strong><small>shendongchun</small></div></div>
      </aside>

      <section className="content">
        <div className="mobile-topbar"><button onClick={() => setMobileNav(!mobileNav)} aria-label="打开菜单"><Menu /></button><strong><AppImage src="/renuxa-logo.svg" alt="Renuxa" width={25} height={25} priority />续序</strong><button onClick={() => setModalOpen(true)} aria-label={t.add}><Plus /></button></div>
        {view === 'dashboard' && <Dashboard subscriptions={subscriptions} bills={bills} currency={baseCurrency} t={t} onAdd={() => setModalOpen(true)} onView={changeView} />}
        {view === 'subscriptions' && <><div className="row-actions"><button title="刷新订阅" aria-label="刷新订阅" onClick={()=>setRefreshVersion(v=>v+1)}><RefreshCw size={18}/></button></div>{refreshError&&<p role="alert">{refreshError}</p>}<SubscriptionsView subscriptions={subscriptions} t={t} onAdd={() => {setEditingSubscription(null);setModalOpen(true);}} onEdit={(sub) => {setEditingSubscription(sub);setModalOpen(true);}} onStatus={updateStatus} onRemove={removeSubscription} /></>}
        {view === 'bills' && <BillsView bills={bills} subscriptions={subscriptions} currency={baseCurrency} token={token} t={t} onUpdate={updateBill} onRefresh={()=>setRefreshVersion(v=>v+1)} refreshError={refreshError} />}
        {view === 'notifications' && <NotificationsView notices={notices} t={t} onRead={readNotice} onReadAll={readAllNotices} />}
        {view === 'settings' && <><SettingsView locale={locale} setLocale={setLocale} currency={baseCurrency} setCurrency={setBaseCurrency} t={t} token={token} userEmail={userEmail} onLogout={() => { setToken(null); setUserEmail(''); }} /><WechatSettings token={token}/></>}
      </section>
      {modalOpen && <SubscriptionModal locale={locale} initial={editingSubscription} onClose={() => {setModalOpen(false);setEditingSubscription(null);}} onSave={editingSubscription ? editSubscription : addSubscription} />}
    </main>
  );
}

function AuthScreen({ onAuthenticated }: { onAuthenticated:(session:{ access_token:string; email:string })=>void }) {
  const [mode,setMode]=useState<'login'|'register'>('login'); const [email,setEmail]=useState(''); const [password,setPassword]=useState(''); const [error,setError]=useState(''); const [busy,setBusy]=useState(false);
  const submit=async(event:FormEvent)=>{event.preventDefault();setBusy(true);setError('');try{const result=await apiRequest<{access_token:string;email:string}>(`/auth/${mode}`,{method:'POST',body:JSON.stringify({email,password})});onAuthenticated(result);}catch(reason){setError(reason instanceof Error?reason.message:'请求失败');}finally{setBusy(false);}};
  return <main className="auth-shell"><section className="auth-brand"><span className="brand-mark"><AppImage src="/renuxa-logo.svg" alt="" width={39} height={39} priority /></span><div><strong>续序</strong><small>Renuxa</small></div><h1>让每一次续费，<br/>都心中有数</h1><p>订阅、账单、汇率和提醒，在同一处保持有序。</p></section><section className="auth-form-wrap"><form className="auth-form" onSubmit={submit}><p>RENuxa ACCOUNT</p><h2>{mode==='login'?'登录续序':'创建账户'}</h2><span>{mode==='login'?'继续管理你的所有订阅':'开始建立清晰的订阅账本'}</span><label className="field"><b>邮箱</b><input type="email" required value={email} onChange={(e)=>setEmail(e.target.value)} placeholder="name@example.com"/></label><label className="field"><b>密码</b><input type="password" required minLength={10} value={password} onChange={(e)=>setPassword(e.target.value)} placeholder="至少 10 位"/></label>{error&&<div className="form-error">{error}</div>}<button className="primary auth-submit" disabled={busy}>{busy?<RefreshCw className="spin"/>:mode==='login'?'登录':'注册'}</button><button className="auth-switch" type="button" onClick={()=>{setMode(mode==='login'?'register':'login');setError('')}}>{mode==='login'?'没有账户？创建一个':'已有账户？返回登录'}</button></form></section></main>;
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
    <section className="notice-band"><span className="notice-icon"><Bell size={17}/></span><div><strong>{upcoming[0] ? `${upcoming[0].name} 将在近期续费` : '暂无近期续费'}</strong><p>{upcoming[0] ? `预计扣款 ${money(upcoming[0].amount,upcoming[0].currency)}，续费提醒已安排。` : '添加订阅后，续序会在到期前提醒你。'}</p></div><button onClick={() => onView('notifications')}>{t.all}</button></section>
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
    for (const billingDate of billingDates(sub, month, monthEnd)) {
      const key = dateKey(billingDate);
      entries.set(key, [...(entries.get(key) ?? []), sub]);
    }
  });
  const todayKey = dateKey(new Date());
  return <section className="panel calendar-panel"><div className="panel-heading"><div><p>SPENDING CALENDAR</p><h2>{month.toLocaleDateString('zh-CN', { year: 'numeric', month: 'long' })}</h2></div><div className="calendar-nav"><button title="上个月" aria-label="上个月" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}><ChevronLeft size={16}/></button><button title="下个月" aria-label="下个月" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}><ChevronRight size={16}/></button></div></div><div className="calendar-grid">{['日','一','二','三','四','五','六'].map((day) => <span className="calendar-weekday" key={day}>{day}</span>)}{Array.from({ length: firstWeekday }, (_, index) => <span className="calendar-empty" key={`empty-${index}`} />)}{Array.from({ length: days }, (_, index) => { const day = index + 1; const date = new Date(month.getFullYear(), month.getMonth(), day); const items = entries.get(dateKey(date)) ?? []; return <div className={`calendar-day ${dateKey(date) === todayKey ? 'today' : ''}`} key={day}><b>{day}</b>{items.map((item) => { const converted = item.amount * (rates[item.currency] ?? 0) / (rates[currency] ?? 1); return <span className="calendar-entry" title={`${item.name} · ${money(item.amount, item.currency)}`} key={`${item.id}-${day}`}><i style={{ background: item.color }} />{item.name}<em>{money(converted, currency)}</em></span>; })}</div>; })}</div></section>;
}

function SubscriptionsView({ subscriptions, t, onAdd, onEdit, onStatus, onRemove }: { subscriptions: Subscription[]; t: typeof copy['zh-CN']; onAdd: () => void; onEdit:(sub:Subscription)=>void; onStatus: (id:string,status:Status)=>void; onRemove:(id:string)=>void }) {
  const [query, setQuery] = useState(''); const [filter, setFilter] = useState<'all'|Status>('all');
  const shown = subscriptions.filter((s) => (filter === 'all' || s.status === filter) && `${s.name} ${s.plan} ${s.category}`.toLowerCase().includes(query.toLowerCase()));
  return <>
    <PageHeader eyebrow="SUBSCRIPTIONS" title={t.subscriptions} description={t.subDesc} action={<button className="primary" onClick={onAdd}><Plus size={17}/>{t.add}</button>} />
    <div className="toolbar"><label className="search-field"><Search size={16}/><input value={query} onChange={(e)=>setQuery(e.target.value)} placeholder={t.search}/></label><label className="filter-select"><SlidersHorizontal size={15}/><select value={filter} onChange={(e)=>setFilter(e.target.value as typeof filter)}><option value="all">全部状态</option><option value="active">使用中</option><option value="paused">已暂停</option><option value="cancelled">已取消</option></select></label></div>
    <section className="data-panel subscriptions-table"><div className="table-head"><span>订阅服务</span><span>分类</span><span>周期</span><span>下次续费</span><span>金额</span><span>状态</span><span /></div>{shown.map((sub)=><article className="table-row" key={sub.id}><div className="service-cell"><ServiceIcon item={sub}/><span><strong>{sub.name}</strong><small>{sub.plan}</small></span></div><span>{sub.category}</span><span>{cadenceLabel(sub)}</span><span>{sub.nextDate}</span><strong>{money(sub.amount,sub.currency)}</strong><StatusBadge status={sub.status}/><div className="row-actions"><button title="编辑" aria-label={`编辑 ${sub.name}`} onClick={()=>onEdit(sub)}><Pencil size={15}/></button>{sub.status === 'active' ? <button title="暂停" onClick={()=>onStatus(sub.id,'paused')}><Pause size={15}/></button> : <button title="恢复" onClick={()=>onStatus(sub.id,'active')}><Play size={15}/></button>}<button title="归档" onClick={()=>onRemove(sub.id)}><Trash2 size={15}/></button></div></article>)}</section>
    {shown.length === 0 && <div className="empty-state"><Search/><strong>没有找到订阅</strong><span>调整搜索或筛选条件后再试。</span></div>}
  </>;
}

function StatusBadge({ status }: { status: Status }) { const labels={active:'使用中',paused:'已暂停',cancelled:'已取消'}; return <span className={`status ${status}`}>{labels[status]}</span>; }
function NotificationsView({ notices, t, onRead, onReadAll }: { notices: Notice[]; t: typeof copy['zh-CN']; onRead:(id:string)=>void; onReadAll:()=>void }) {
  return <><PageHeader eyebrow="INBOX" title={t.notifications} description={t.noticesDesc} action={<button className="secondary" onClick={onReadAll} disabled={!notices.some((notice) => !notice.read)}><Check size={16}/>{t.markAll}</button>}/>{notices.length > 0 ? <section className="notification-list">{notices.map((notice)=><button key={notice.id} className={`notification-item ${notice.read?'read':''}`} onClick={()=>onRead(notice.id)}><span className={`notification-kind ${notice.kind}`}>{notice.kind==='renewal'?<RefreshCw/>:notice.kind==='bill'?<ReceiptText/>:<Globe2/>}</span><span className="notification-copy"><strong>{notice.title}</strong><small>{notice.body}</small></span><time>{notice.date}</time>{!notice.read&&<i/>}</button>)}</section> : <div className="empty-state page-empty"><Bell/><strong>暂无通知</strong><span>续费提醒和账单消息会显示在这里。</span></div>}</>;
}

const defaultNotificationSettings: NotificationSettings = {
  telegram_enabled: false, telegram_bot_token_configured: false, telegram_chat_id: '',
};

function SettingsView({ locale, setLocale, currency, setCurrency, t, token, userEmail, onLogout }: { locale:Locale; setLocale:(v:Locale)=>void; currency:string; setCurrency:(v:string)=>void; t:typeof copy['zh-CN']; token:string|null; userEmail:string; onLogout:()=>void }) {
  const [tab,setTab]=useState<'general'|'notifications'|'security'>('general');
  const [timezone,setTimezone]=useStoredState('renuxa.timezone','Asia/Shanghai');
  const [reminders,setReminders]=useStoredState<number[]>('renuxa.reminders',[7,3,1]);
  const [notificationSettings,setNotificationSettings]=useState(defaultNotificationSettings);
  const [telegramToken,setTelegramToken]=useState('');
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
      })},token) as NotificationSettings;
      setNotificationSettings(saved); setTelegramToken(''); setSaveState('saved');
    } catch(reason) {
      setSaveState('error'); setSaveError(reason instanceof Error?reason.message:'保存失败');
    }
  };

  return <><PageHeader eyebrow="PREFERENCES" title={t.settings} description={t.settingsDesc}/><div className="settings-layout"><nav className="settings-nav" aria-label="设置分类"><button className={tab==='general'?'active':''} onClick={()=>setTab('general')}><Globe2/>通用</button><button className={tab==='notifications'?'active':''} onClick={()=>setTab('notifications')}><Bell/>通知</button><button className={tab==='security'?'active':''} onClick={()=>setTab('security')}><ShieldCheck/>账户与安全</button></nav><section className="settings-content">{tab==='general'&&<div className="settings-group"><h2>显示与地区</h2><SettingRow title="界面语言" description="更改界面中的文字语言"><select aria-label="界面语言" value={locale} onChange={(e)=>setLocale(e.target.value as Locale)}><option value="zh-CN">简体中文</option><option value="en">English</option></select></SettingRow><SettingRow title="基准货币" description="仪表盘和统计的默认折算货币"><select aria-label="基准货币" value={currency} onChange={(e)=>setCurrency(e.target.value)}>{Object.keys(rates).map((code)=><option key={code}>{code}</option>)}</select></SettingRow><SettingRow title="时区" description="用于界面中的日期和时间"><select aria-label="时区" value={timezone} onChange={(e)=>setTimezone(e.target.value)}><option>Asia/Shanghai</option><option>Asia/Hong_Kong</option><option>America/New_York</option><option>Europe/London</option></select></SettingRow></div>}{tab==='notifications'&&<form className="settings-group" onSubmit={saveNotificationSettings}><h2>通知渠道</h2><SettingRow title="应用内通知" description="续费提醒始终保留在通知中心"><span className="setting-value">始终启用</span></SettingRow><div className="notification-channel"><div className="channel-heading"><span className="channel-icon telegram"><Send/></span><div><strong>Telegram</strong><small>通过机器人发送续费提醒</small></div><button type="button" className={`toggle ${notificationSettings.telegram_enabled?'on':''}`} aria-label="启用 Telegram" aria-pressed={notificationSettings.telegram_enabled} onClick={()=>updateNotificationSetting('telegram_enabled',!notificationSettings.telegram_enabled)}><span/></button></div>{notificationSettings.telegram_enabled&&<div className="channel-fields"><label className="field"><span>Bot Token</span><input type="password" autoComplete="new-password" value={telegramToken} onChange={(e)=>setTelegramToken(e.target.value)} placeholder={notificationSettings.telegram_bot_token_configured?'已配置，留空保持不变':'从 BotFather 获取'}/></label><label className="field"><span>Chat ID</span><input value={notificationSettings.telegram_chat_id} onChange={(e)=>updateNotificationSetting('telegram_chat_id',e.target.value)} placeholder="例如：123456789"/></label></div>}</div><div className="reminder-row"><div><strong>提前提醒</strong><small>新订阅默认使用，可在单项中覆盖</small></div><div className="reminder-chips">{[14,7,3,1].map((day)=><button type="button" aria-pressed={reminders.includes(day)} key={day} className={reminders.includes(day)?'active':''} onClick={()=>setReminders(reminders.includes(day)?reminders.filter((v)=>v!==day):[...reminders,day].sort((a,b)=>b-a))}>{day} 天</button>)}</div></div><div className="settings-actions"><span className={saveState==='error'?'save-error':'save-status'}>{saveError||(saveState==='saved'?'设置已保存':'')}</span><button className="primary" disabled={!token||saveState==='saving'}>{saveState==='saving'?<RefreshCw className="spin"/>:<Check/>}保存设置</button></div></form>}{tab==='security'&&<div className="settings-group"><h2>账户与安全</h2><SettingRow title="当前账户" description={userEmail||'已连接 Renuxa 服务端'}><span className="setting-value">已登录</span></SettingRow><SettingRow title="退出登录" description="此设备上的订阅数据将在再次登录后同步"><button className="secondary" onClick={onLogout}>退出登录</button></SettingRow></div>}</section></div></>;
}

function WechatSettings({token}:{token:string|null}) {
  const [status,setStatus]=useState<{enabled:boolean;bound:boolean}|null>(null);
  const [code,setCode]=useState('');
  const [qrStatus,setQrStatus]=useState('');
  const [expires,setExpires]=useState('');
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState('');
  useEffect(()=>{
    if(!token) return;
    let active=true;
    const load=()=>apiRequest<{enabled:boolean;bound:boolean}>('/integrations/wechat/binding',{},token).then(value=>{if(active){setStatus(value);if(value.bound)setCode('');}}).catch(reason=>{if(active)setError(reason.message);});
    void load();
    const timer=window.setInterval(()=>{void load();},15000);
    return ()=>{active=false;window.clearInterval(timer);};
  },[token]);
  useEffect(()=>{
    if(!code||!token) return;
    let active=true;
    let timer:ReturnType<typeof setTimeout>;
    const poll=async()=>{
      try {
        const value=await apiRequest<{status:string}>('/integrations/wechat/qrcode/status',{},token);
        if(!active)return;
        if(value.status==='confirmed'){
          setCode('');setQrStatus('绑定成功');setStatus({enabled:true,bound:true});return;
        }
        if(['expired','idle','stale'].includes(value.status)){
          setCode('');setQrStatus('二维码已过期，请重新获取');return;
        }
        setQrStatus(value.status==='scaned'||value.status==='scanned'?'已扫码，请在微信确认':'等待微信扫码');
        timer=setTimeout(()=>void poll(),2000);
      }catch(reason){if(active){setError(reason instanceof Error?reason.message:'扫码状态查询失败');setCode('');}}
    };
    timer=setTimeout(()=>void poll(),2000);
    const expiry=setTimeout(()=>{if(active){active=false;clearTimeout(timer);setCode('');setQrStatus('二维码已过期，请重新获取');}},300000);
    return ()=>{active=false;clearTimeout(timer);clearTimeout(expiry);};
  },[code,token]);
  const action=async(unbind:boolean)=>{
    setBusy(true);setError('');
    try {
      const timezone=JSON.parse(localStorage.getItem('renuxa.timezone')??'"Asia/Shanghai"');
      const result=await apiRequest<{image:string;expires_in:number}>(`/integrations/wechat/${unbind?'binding':'qrcode'}`,{method:unbind?'DELETE':'POST',body:unbind?undefined:JSON.stringify({timezone})},token);
      setCode(unbind?'':result.image);
      setQrStatus(unbind?'':'等待微信扫码');
      setExpires(unbind?'':new Date(Date.now()+result.expires_in*1000).toLocaleTimeString());
      setStatus(await apiRequest<{enabled:boolean;bound:boolean}>('/integrations/wechat/binding',{},token));
    } catch(reason){setError(reason instanceof Error?reason.message:'操作失败');}
    finally{setBusy(false);}
  };
  return <section className="settings-group wechat-settings"><h2>微信接入</h2><SettingRow title={status?.bound?'已绑定':status?.enabled?'未绑定':'未启用'} description={code?`有效期至 ${expires}`:''}><button className="secondary" disabled={!token||!status?.enabled||busy} onClick={()=>void action(Boolean(status?.bound))}>{status?.bound?<Trash2 size={16}/>:<Plus size={16}/>} {busy?'处理中':status?.bound?'解绑':code?'刷新二维码':'扫码绑定微信'}</button></SettingRow>{code&&<div className="wechat-qr"><AppImage src={code} alt="微信绑定二维码" width={240} height={240} /></div>}{qrStatus&&<p role="status">{qrStatus}</p>}{error&&<p className="save-error" role="alert">{error}</p>}</section>;
}

function SettingRow({ title, description, children }: { title:string; description:string; children:React.ReactNode }) { return <div className="setting-row"><div><strong>{title}</strong><small>{description}</small></div>{children}</div>; }

function SubscriptionModal({ locale, initial, onClose, onSave }: { locale:Locale; initial:Subscription|null; onClose:()=>void; onSave:(sub:Subscription)=>Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [iconCandidates, setIconCandidates] = useState<{name:string; developer:string; icon_url:string; bundle_id:string}[]>([]);
  const [iconMessage, setIconMessage] = useState('');
  const iconSearch = useRef({ version: 0, query: '' });
  const [name,setName]=useState(initial?.name??''); const [plan,setPlan]=useState(initial?.plan??''); const [amount,setAmount]=useState(initial?String(initial.amount):''); const [currency,setCurrency]=useState(initial?.currency??'CNY'); const [cadence,setCadence]=useState(cadenceUnit(initial?.cadence??'month')); const [cadenceInterval,setCadenceInterval]=useState(String(initial?.cadenceInterval??1)); const [nextDate,setNextDate]=useState(initial?.nextDate??'2026-09-30'); const [category,setCategory]=useState(initial?.category??'工作效率'); const [iconUrl,setIconUrl]=useState(initial?.iconUrl??''); const [iconBusy,setIconBusy]=useState(false); const [reminders,setReminders]=useState<number[]>(()=>initial?.reminderOffsets??(()=>{try{return JSON.parse(localStorage.getItem('renuxa.reminders')??'[7,3,1]')}catch{return [7,3,1]}})());
  const changeName = (value: string) => {
    setName(value);
    iconSearch.current = { version: iconSearch.current.version + 1, query: '' };
    setIconCandidates([]);setIconUrl('');setIconMessage('');setIconBusy(false);
  };
  const searchIcon=async()=>{
    const query = name.trim();
    if(!query || iconSearch.current.query === query)return;
    const version = ++iconSearch.current.version;
    iconSearch.current.query = query;
    setIconBusy(true);setIconMessage('');
    try {
      const results = await apiRequest<{name:string; developer:string; icon_url:string; bundle_id:string}[]>(`/icons/search?q=${encodeURIComponent(query)}`);
      if (version !== iconSearch.current.version) return;
      setIconCandidates(results);setIconUrl(results[0]?.icon_url ?? '');
      if (!results.length) setIconMessage('未找到匹配图标');
    } catch {
      if (version !== iconSearch.current.version) return;
      iconSearch.current.query = '';
      setIconMessage('图标搜索失败，请重试');
    } finally {if (version === iconSearch.current.version) setIconBusy(false);}
  };
  const submit=async(event:FormEvent)=>{
    event.preventDefault();
    if (busy) return;
    const numeric=Number(amount);
    const interval=Number(cadenceInterval);
    if(!name.trim()||!amount.trim()||!Number.isFinite(numeric)||numeric<0){setError('请输入订阅名称和有效的非负金额');return;}
    if(!Number.isInteger(interval)||interval<1||interval>120){setError('周期倍数必须是 1 到 120 的整数');return;}
    setBusy(true);setError('');
    try {
      await onSave({id:initial?.id??createLocalId(),name:name.trim(),plan:plan.trim()||'标准方案',amount:numeric,currency,cadence,nextDate,category,status:initial?.status??'active',color:initial?.color??colors[name.length%colors.length],iconUrl:iconUrl||undefined,reminderOffsets:reminders,cadenceInterval:cadence==='once'?1:interval,anchorDay:initial?.anchorDay});
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : `${initial?'保存':'添加'}失败，请重试`);
    } finally {setBusy(false);}
  };
  const editing=Boolean(initial);
  return <div className="modal-backdrop" role="presentation" onMouseDown={(e)=>{if(e.currentTarget===e.target)onClose();}}><section className="modal" role="dialog" aria-modal="true" aria-labelledby="subscription-modal-title"><header><div><p>{editing?'EDIT SUBSCRIPTION':'NEW SUBSCRIPTION'}</p><h2 id="subscription-modal-title">{locale==='zh-CN'?(editing?'编辑订阅':'添加订阅'):(editing?'Edit subscription':'Add subscription')}</h2></div><button onClick={onClose} aria-label="关闭"><X/></button></header><form onSubmit={submit}><div className="icon-name-row"><ServiceIcon size="large" item={{name:name||'?',color:initial?.color??colors[name.length%colors.length],iconUrl:iconUrl||undefined}}/><label className="field grow"><span>订阅名称</span><div className="input-with-action"><input autoFocus required value={name} onChange={(e)=>changeName(e.target.value)} onBlur={searchIcon} placeholder="例如：Spotify"/><button type="button" onClick={searchIcon} title="从 App Store 匹配图标">{iconBusy?<RefreshCw className="spin"/>:<Search/>}</button></div></label></div>
    {iconBusy && <p className="icon-search-message" role="status">正在搜索图标...</p>}
    {iconMessage && <p className="icon-search-message" role="status">{iconMessage}</p>}
    {iconCandidates.length > 0 && <fieldset className="icon-candidates"><legend>订阅图标</legend><div className="icon-candidate-grid">{iconCandidates.map((candidate, index) => <label className="icon-candidate" key={candidate.bundle_id + index} title={candidate.name + ' · ' + candidate.developer}><input type="radio" name="subscription-icon" value={candidate.icon_url} checked={iconUrl === candidate.icon_url} onChange={() => setIconUrl(candidate.icon_url)} /><ServiceIcon item={{name:candidate.name,color:colors[index % colors.length],iconUrl:candidate.icon_url}}/><span><strong>{candidate.name}</strong><small>{candidate.developer}</small></span></label>)}</div></fieldset>}
    <div className="form-grid"><label className="field span-2"><span>方案名称</span><input value={plan} onChange={(e)=>setPlan(e.target.value)} placeholder="例如：个人高级版"/></label><label className="field"><span>金额</span><input required inputMode="decimal" value={amount} onChange={(e)=>setAmount(e.target.value)} placeholder="0.00"/></label><label className="field"><span>货币</span><select value={currency} onChange={(e)=>setCurrency(e.target.value)}>{Object.keys(rates).map((code)=><option key={code}>{code}</option>)}</select></label><label className="field"><span>扣费周期</span><select value={cadence} onChange={(e)=>setCadence(e.target.value)}><option value="day">天</option><option value="week">周</option><option value="month">月</option><option value="quarter">季度</option><option value="year">年</option><option value="once">一次性</option></select></label><label className="field"><span>周期倍数</span><input type="number" min="1" max="120" required disabled={cadence==='once'} value={cadence==='once'?'1':cadenceInterval} onChange={(e)=>setCadenceInterval(e.target.value)}/></label><label className="field"><span>下次续费</span><input type="date" required value={nextDate} onChange={(e)=>setNextDate(e.target.value)}/></label><label className="field span-2"><span>分类</span><select value={category} onChange={(e)=>setCategory(e.target.value)}><option>工作效率</option><option>影音娱乐</option><option>云服务</option><option>学习教育</option><option>健康生活</option><option>其他</option></select></label></div><div className="reminder-config"><span><Bell size={15}/>提前提醒</span><div>{[14,7,3,1].map((day)=><button type="button" key={day} className={reminders.includes(day)?'active':''} onClick={()=>setReminders(reminders.includes(day)?reminders.filter((v)=>v!==day):[...reminders,day])}>{day} 天</button>)}</div></div>{error&&<div className="form-error" role="alert">{error}</div>}<footer><button className="secondary" type="button" onClick={onClose}>取消</button><button className="primary" type="submit" disabled={busy}>{busy?<RefreshCw className="spin"/>:editing?<Check size={16}/>:<Plus size={16}/>} {editing?'保存修改':'添加订阅'}</button></footer></form></section></div>;
}
