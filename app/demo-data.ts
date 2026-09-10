import type { Subscription, Bill, Notice } from './models';

export const seedSubscriptions: Subscription[] = [
  { id: 'sub-figma', name: 'Figma', plan: 'Professional', amount: 15, currency: 'USD', cadence: 'monthly', nextDate: '2026-09-03', category: '工作效率', status: 'active', color: '#242424' },
  { id: 'sub-icloud', name: 'iCloud+', plan: '2 TB', amount: 68, currency: 'CNY', cadence: 'monthly', nextDate: '2026-09-06', category: '云服务', status: 'active', color: '#2688e6' },
  { id: 'sub-netflix', name: 'Netflix', plan: '标准套餐', amount: 73, currency: 'HKD', cadence: 'monthly', nextDate: '2026-09-12', category: '影音娱乐', status: 'active', color: '#e5232b' },
  { id: 'sub-notion', name: 'Notion', plan: 'Plus', amount: 10, currency: 'USD', cadence: 'monthly', nextDate: '2026-09-18', category: '工作效率', status: 'active', color: '#353535' },
  { id: 'sub-applemusic', name: 'Apple Music', plan: '个人', amount: 11, currency: 'CNY', cadence: 'monthly', nextDate: '2026-09-22', category: '影音娱乐', status: 'active', color: '#ef4962' },
  { id: 'sub-dropbox', name: 'Dropbox', plan: 'Plus', amount: 119.88, currency: 'USD', cadence: 'yearly', nextDate: '2027-01-14', category: '云服务', status: 'paused', color: '#1877f2' },
];

export const seedBills: Bill[] = [
  { id: 'bill-1', subscription: 'iCloud+', date: '2026-08-06', amount: 68, currency: 'CNY', status: 'paid' },
  { id: 'bill-2', subscription: 'Figma', date: '2026-08-03', amount: 15, currency: 'USD', status: 'paid' },
  { id: 'bill-3', subscription: 'Netflix', date: '2026-08-12', amount: 73, currency: 'HKD', status: 'paid' },
  { id: 'bill-4', subscription: 'Notion', date: '2026-09-18', amount: 10, currency: 'USD', status: 'estimated' },
  { id: 'bill-5', subscription: 'Apple Music', date: '2026-09-22', amount: 11, currency: 'CNY', status: 'estimated' },
];

export const seedNotices: Notice[] = [
  { id: 'n-1', title: 'Figma 将在 2 天后续费', body: '预计扣款 US$15.00，到期提醒已安排。', date: '今天 09:00', read: false, kind: 'renewal' },
  { id: 'n-2', title: 'iCloud+ 账单已确认', body: '8 月账单 CN¥68.00 已计入支出统计。', date: '8月6日', read: false, kind: 'bill' },
  { id: 'n-3', title: '每日汇率已更新', body: '当前汇率数据日期为 2026 年 8 月 31 日。', date: '昨天', read: false, kind: 'system' },
];

