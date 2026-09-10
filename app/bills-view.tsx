'use client';

import { useEffect, useState } from 'react';
import { CalendarDays, Check, CircleDollarSign, ReceiptText, RefreshCw, WalletCards } from 'lucide-react';
import { apiRequest, apiUrl } from './api';
import { billSummary, convertBillAmount, upcomingBills, historicalBillAmount } from './billing';
import { rates as demoRates } from './constants';
import { money } from './format';
import type { copy } from './copy';
import type { Bill, BillStatus, Subscription } from './models';

const labels = { estimated: '待确认', paid: '已支付', skipped: '未扣款', refunded: '已退款' };
const demoExchangeRates = Object.fromEntries(Object.entries(demoRates).map(([currency, rate]) => [currency, 1 / rate]));

type Props = {
  bills: Bill[];
  subscriptions: Subscription[];
  currency: string;
  token: string | null;
  t: typeof copy['zh-CN'];
  onUpdate: (id: string, status: BillStatus) => Promise<void>;
  onRefresh: () => void;
  refreshError: string;
};

export function BillsView({ bills, subscriptions, currency, token, t, onUpdate, onRefresh, refreshError }: Props) {
  const [filter, setFilter] = useState<'all' | BillStatus>('all');
  const [pendingId, setPendingId] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [exchangeRates, setExchangeRates] = useState<Record<string, number>>(apiUrl ? {} : demoExchangeRates);
  const [rateDate, setRateDate] = useState('');
  const [rateRefresh, setRateRefresh] = useState(0);
  useEffect(() => {
    if (!apiUrl || !token) return;
    let cancelled = false;
    apiRequest<{ base: string; rates: { currency: string; rate: string | number; date: string }[] }>('/exchange-rates', {}, token)
      .then((result) => {
        if (cancelled) return;
        setExchangeRates(Object.fromEntries([[result.base, 1], ...result.rates.map((rate) => [rate.currency, Number(rate.rate)])]));
        setRateDate(result.rates[0]?.date ?? '');
      }).catch(() => { /* Keep the last successfully loaded rates during a transient failure. */ });
    return () => { cancelled = true; };
  }, [token, rateRefresh]);

  const shown = bills.filter((bill) => filter === 'all' || bill.status === filter);
  const { paidThisYear, pending } = billSummary(bills);
  const originalPaid = Object.entries(paidThisYear.reduce<Record<string,number>>((totals,bill)=>{
    totals[bill.currency]=(totals[bill.currency]??0)+bill.amount;
    return totals;
  },{})).map(([code,amount])=>money(amount,code)).join(' + ');
  const forecasts = upcomingBills(subscriptions, bills);
  const converted = (bill: { amount: number; currency: string }) => apiUrl && 'status' in bill ? historicalBillAmount(bill as Bill, currency) : convertBillAmount(bill.amount, bill.currency, currency, exchangeRates);
  const paidAmounts = paidThisYear.map(converted);
  const total = paidAmounts.some((value) => value === null) ? null : paidAmounts.reduce<number>((sum, value) => sum + (value ?? 0), 0);
  const displayConverted = (bill: { amount: number; currency: string }) => {
    const value = converted(bill);
    return value === null ? '汇率待同步' : money(value, currency);
  };
  const confirmPayment = async (id: string) => {
    setPendingId(id);
    setError('');
    try { await onUpdate(id, 'paid'); }
    catch (reason) { setError(reason instanceof Error ? reason.message : '确认支付失败，请重试'); }
    finally { setPendingId(null); }
  };

  return <>
    <header className="topbar"><div><p>BILLING</p><h1>{t.bills}</h1><span className="page-description">{t.billsDesc}</span></div><button className="secondary" onClick={()=>{onRefresh();setRateRefresh(value=>value+1);}}><RefreshCw size={16}/>刷新账单</button></header>
    {(refreshError || error) && <p role="alert">{error || refreshError}</p>}
    {subscriptions.some(sub=>!sub.startDate)&&<p role="status">部分订阅尚未设置开始日期。请在“我的订阅”编辑并补填，以生成历史已支付账单。</p>}
    <div className="bill-summary">
      <div><CircleDollarSign/><span>今年已支付<small>按账单日 · {currency}</small></span><strong>{total === null ? '汇率待同步' : money(total, currency)}</strong></div>
      <div><CalendarDays/><span>待确认账单<small>已生成，含逾期</small></span><strong>{pending}</strong></div>
      <div><WalletCards/><span>记录总数<small>全部账单</small></span><strong>{bills.length}</strong></div>
    </div>
    <p className="page-description">{apiUrl ? (rateDate ? `扣款预估使用 ${rateDate} 参考汇率；已入账记录使用账单日对应的历史汇率。` : '暂无参考汇率；原币金额正常显示。') : '离线演示数据，折算使用示例汇率。'}</p>
    {total === null && <p>今年已支付（原币合计）：{originalPaid}</p>}
    <div className="segmented">{(['all', 'estimated', 'paid', 'skipped', 'refunded'] as const).map((value) => <button className={filter === value ? 'active' : ''} key={value} onClick={() => setFilter(value)}>{value === 'all' ? '全部' : labels[value]}</button>)}</div>
    <section className="data-panel bills-table" aria-label="已生成账单">
      <div className="table-head"><span>订阅</span><span>账单日</span><span>原币金额</span><span>折合 {currency}</span><span>状态</span><span/></div>
      {shown.map((bill) => <article className="table-row" key={bill.id}>
        <strong>{bill.subscription}</strong><span>{bill.date}</span><strong>{money(bill.amount, bill.currency)}</strong><span>{displayConverted(bill)}{bill.exchangeRateDate&&<small>汇率日 {bill.exchangeRateDate}</small>}</span><span className={`status ${bill.status}`}>{labels[bill.status]}</span>
        <div className="bill-action">{bill.status === 'estimated' && <button disabled={pendingId !== null} onClick={() => void confirmPayment(bill.id)}><Check size={14}/>{pendingId === bill.id ? '保存中…' : '确认支付'}</button>}</div>
      </article>)}
    </section>
    {shown.length === 0 && <div className="empty-state"><ReceiptText/><strong>暂无账单记录</strong><span>{filter === 'all' ? '订阅到期后自动生成账单；未来扣款预估见下方。' : '当前筛选条件下没有账单。'}</span></div>}
    <section aria-label="未来 30 天扣款预估">
      <h2>未来 30 天扣款预估 · {forecasts.length} 笔</h2>
      <p className="page-description">根据有效订阅的下次扣款日和周期计算；预估尚未入账，不计入已支付或待确认账单。</p>
      {forecasts.length > 0 ? <div className="data-panel bills-table">
        <div className="table-head"><span>订阅</span><span>预计扣款日</span><span>原币金额</span><span>折合 {currency}</span><span>状态</span><span/></div>
        {forecasts.map((bill) => <article className="table-row" key={bill.id}><strong>{bill.subscription}</strong><span>{bill.date}</span><strong>{money(bill.amount, bill.currency)}</strong><span>{displayConverted(bill)}</span><span className="status estimated">未入账</span><span/></article>)}
      </div> : <p>未来 30 天没有待入账的扣款预估。</p>}
    </section>
  </>;
}
