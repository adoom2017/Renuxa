import assert from 'node:assert/strict';
import test from 'node:test';
// @ts-expect-error Node's native TypeScript loader needs the explicit extension.
import { billingDates, cadenceLabel } from './billing.ts';

const date=(value:string)=>new Date(`${value}T00:00:00`);
test('all cadences and intervals count scheduled charges',()=>{
  for(const [cadence,interval,count] of [['day',2,15],['week',2,3],['month',1,1],['quarter',1,1],['year',1,1],['once',1,1]] as const) {
    assert.equal(billingDates({cadence,cadenceInterval:interval,nextDate:'2026-09-01'},date('2026-09-01'),date('2026-10-01')).length,count);
  }
  assert.equal(billingDates({cadence:'once',nextDate:'2026-08-31'},date('2026-09-01'),date('2026-10-01')).length,0);
  assert.equal(cadenceLabel({cadence:'week',cadenceInterval:2,nextDate:''}),'每2周');
});
test('month-end anchor survives February and leap years',()=>{
  const values=billingDates({cadence:'month',nextDate:'2026-01-31',anchorDay:31},date('2026-01-01'),date('2026-04-01'));
  assert.deepEqual(values.map((v:Date)=>v.getDate()),[31,28,31]);
  const leap=billingDates({cadence:'year',nextDate:'2024-02-29',anchorDay:29},date('2024-01-01'),date('2029-01-01'));
  assert.deepEqual(leap.map((v:Date)=>v.getDate()),[29,28,28,28,29]);
});
test('legacy local cadence and quarter intervals remain compatible',()=>{
  assert.equal(cadenceLabel({cadence:'monthly',nextDate:''}),'每月');
  assert.equal(billingDates({cadence:'quarter',cadenceInterval:2,nextDate:'2026-01-31'},date('2026-01-01'),date('2027-01-01')).length,2);
});

// @ts-expect-error Node's native TypeScript loader needs the explicit extension.
import { billSummary, upcomingBills, convertBillAmount } from './billing.ts';
import type { Bill, Subscription } from './models';

const subscription = (overrides: Partial<Subscription> = {}): Subscription => ({
  id: 'monthly', name: 'Monthly', plan: '', amount: 10, currency: 'USD', cadence: 'month',
  nextDate: '2026-09-23', category: '', status: 'active', color: '', ...overrides,
});
const bill = (overrides: Partial<Bill> = {}): Bill => ({
  id: 'bill', subscriptionId: 'monthly', subscription: 'Monthly', date: '2026-09-10', amount: 10, currency: 'USD', status: 'paid', ...overrides,
});

test('paid summary excludes other years and refunds; pending includes overdue bills', () => {
  const paid = bill();
  const summary = billSummary([paid, bill({date:'2025-12-31'}), bill({date:'2027-01-01'}), bill({status:'refunded'}), bill({status:'estimated',date:'2025-01-01'})], date('2026-09-10'));
  assert.deepEqual(summary.paidThisYear, [paid]);
  assert.equal(summary.pending, 1);
});

test('forecast uses future subscriptions without creating history before the next date', () => {
  const forecasts = upcomingBills([
    subscription(), subscription({id:'later',nextDate:'2026-11-12'}),
    subscription({id:'paused',status:'paused'}), subscription({id:'cancelled',status:'cancelled'}),
    subscription({id:'boundary',cadence:'once',nextDate:'2026-10-10'}),
    subscription({id:'today',cadence:'once',nextDate:'2026-09-10'}),
  ], [], date('2026-09-10'));
  assert.deepEqual(forecasts.map((item) => item.date), ['2026-09-10', '2026-09-23']);
});

test('forecast respects month-end anchors and excludes already generated bills by subscription id', () => {
  const sub = subscription({nextDate:'2026-01-31',anchorDay:31});
  assert.deepEqual(upcomingBills([sub], [], date('2026-02-01')).map((item) => item.date), ['2026-02-28']);
  assert.equal(upcomingBills([subscription()], [bill({date:'2026-09-23'})], date('2026-09-10')).length, 0);
  assert.equal(upcomingBills([subscription()], [bill({subscriptionId:'different',date:'2026-09-23'})], date('2026-09-10')).length, 1);
});

test('currency conversion uses cross rates and never presents missing rates as zero', () => {
  assert.equal(convertBillAmount(10, 'USD', 'CNY', {EUR:1,USD:1.25,CNY:8.75}), 70);
  assert.equal(convertBillAmount(10, 'CNY', 'CNY', {}), 10);
  assert.equal(convertBillAmount(10, 'USD', 'CNY', {}), null);
  assert.equal(convertBillAmount(10, 'USD', 'CNY', {USD:0,CNY:7}), null);
});
