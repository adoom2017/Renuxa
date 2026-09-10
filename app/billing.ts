import type { Bill, Subscription } from './models';

type BillingSchedule = { cadence: string; nextDate: string; cadenceInterval?: number; anchorDay?: number; startDate?: string };

function addBillingMonths(date: Date, months: number) {
  const day = date.getDate();
  const result = new Date(date.getFullYear(), date.getMonth() + months, 1);
  const lastDay = new Date(result.getFullYear(), result.getMonth() + 1, 0).getDate();
  result.setDate(Math.min(day, lastDay));
  return result;
}

export function cadenceUnit(cadence: string) {
  return ({monthly:'month',quarterly:'quarter',yearly:'year'} as Record<string,string>)[cadence] ?? cadence;
}
export function cadenceLabel(sub: BillingSchedule) {
  const unit=cadenceUnit(sub.cadence);
  if(unit==='once') return '一次性';
  const label=({day:'天',week:'周',month:'月',quarter:'季度',year:'年'} as Record<string,string>)[unit];
  return label ? `每${(sub.cadenceInterval??1)===1?'':sub.cadenceInterval}${label}` : '未知周期';
}
export function billingDates(sub: BillingSchedule, start: Date, end: Date) {
  if (sub.startDate) {
    const first = new Date(`${sub.startDate}T00:00:00`);
    if (first > start) start = first;
  }
  const origin=new Date(`${sub.nextDate}T00:00:00`);
  if(Number.isNaN(origin.getTime())) return [];
  const unit=cadenceUnit(sub.cadence);
  if(unit==='once') return origin>=start&&origin<end?[origin]:[];
  const interval=Math.max(1,sub.cadenceInterval??1);
  const months=({month:1,quarter:3,year:12} as Record<string,number>)[unit];
  if(!months && unit!=='day' && unit!=='week') return [];
  const occurrence=(index:number)=>{
    if(months) {
      const date=addBillingMonths(origin,index*months*interval);
      const last=new Date(date.getFullYear(),date.getMonth()+1,0).getDate();
      date.setDate(Math.min(sub.anchorDay||origin.getDate(),last));
      return date;
    }
    return new Date(origin.getFullYear(),origin.getMonth(),origin.getDate()+index*interval*(unit==='week'?7:1));
  };
  let index=months ? Math.floor(((start.getFullYear()-origin.getFullYear())*12+start.getMonth()-origin.getMonth())/(months*interval))-1 : Math.floor((start.getTime()-origin.getTime())/(86400000*interval*(unit==='week'?7:1)))-2;
  const dates:Date[]=[];
  for(let date=occurrence(index);date<end;date=occurrence(++index)) if(date>=start) dates.push(date);
  return dates;
}


export function billSummary(bills: Bill[], now = new Date()) {
  const year = String(now.getFullYear());
  return {
    paidThisYear: bills.filter((bill) => bill.status === 'paid' && bill.date.slice(0, 4) === year),
    pending: bills.filter((bill) => bill.status === 'estimated').length,
  };
}

// Forecasts are display-only. Only the server's generated bills can be confirmed.
export function upcomingBills(subscriptions: Subscription[], bills: Bill[], now = new Date()) {
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const end = new Date(start.getFullYear(), start.getMonth(), start.getDate() + 30);
  const recorded = new Set(bills.map((bill) => `${bill.subscriptionId}:${bill.date}`));
  return subscriptions.filter((sub) => sub.status === 'active').flatMap((sub) => {
    const next = new Date(`${sub.nextDate}T00:00:00`);
    // billingDates also projects historical periods for dashboard charts; forecasts must not.
    const from = next > start ? next : start;
    return billingDates(sub, from, end).map((date) => {
      const key = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
      return { id: `${sub.id}:${key}`, subscriptionId: sub.id, subscription: sub.name, date: key, amount: sub.amount, currency: sub.currency };
    }).filter((bill) => !recorded.has(bill.id));
  }).sort((a, b) => a.date.localeCompare(b.date) || a.subscription.localeCompare(b.subscription));
}

export function convertBillAmount(amount: number, source: string, target: string, exchangeRates: Record<string, number>): number | null {
  if (source === target) return amount;
  const from = exchangeRates[source];
  const to = exchangeRates[target];
  return Number.isFinite(from) && from > 0 && Number.isFinite(to) && to > 0 ? amount / from * to : null;
}

export function localDateKey(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth()+1).padStart(2,'0')}-${String(date.getDate()).padStart(2,'0')}`;
}

export function subscriptionPlan(sub: BillingSchedule, now = new Date()) {
  if (!sub.startDate) return { paidDates: [] as string[], nextDate: sub.nextDate };
  const first = new Date(`${sub.startDate}T00:00:00`);
  if (Number.isNaN(first.getTime())) return { paidDates: [] as string[], nextDate: sub.nextDate };
  const end = new Date(now.getFullYear(), now.getMonth(), now.getDate()+1);
  const dates = billingDates({...sub, nextDate: sub.startDate, anchorDay: first.getDate()}, first, end);
  let next = dates.at(-1) ?? first;
  const unit = cadenceUnit(sub.cadence);
  if (dates.length && unit !== 'once') {
    const interval = sub.cadenceInterval ?? 1;
    const months = ({month:1,quarter:3,year:12} as Record<string,number>)[unit];
    if (months) {
      next = addBillingMonths(next, months*interval);
      next.setDate(Math.min(first.getDate(),new Date(next.getFullYear(),next.getMonth()+1,0).getDate()));
    } else next = new Date(next.getFullYear(),next.getMonth(),next.getDate()+interval*(unit==='week'?7:1));
  }
  return { paidDates: dates.map(localDateKey), nextDate: localDateKey(next) };
}

export function localSubscriptionBills(sub: Subscription, existing: Bill[]) {
  if (!sub.startDate) return existing;
  const {paidDates} = subscriptionPlan(sub);
  const dates = new Set(paidDates);
  const prefix = `schedule:${sub.id}:`;
  const kept = existing.filter((bill)=>bill.source==='manual'||!bill.id.startsWith(prefix)||dates.has(bill.date));
  const recorded = new Set(kept.map((bill)=>`${bill.subscriptionId}:${bill.date}`));
  return [...kept, ...paidDates.filter((date)=>!recorded.has(`${sub.id}:${date}`)).map((date): Bill=>({
    id: `${prefix}${date}`, subscriptionId: sub.id, subscription: sub.name, date,
    amount: sub.amount, currency: sub.currency, status: 'paid', source: 'schedule',
  }))].sort((a,b)=>b.date.localeCompare(a.date));
}

export function historicalBillAmount(bill: Bill, target: string): number | null {
  if (bill.currency === target) return bill.amount;
  if (bill.baseCurrency === target && bill.baseAmount !== undefined) return bill.baseAmount;
  return convertBillAmount(bill.amount, bill.currency, target, bill.referenceRates ?? {});
}
