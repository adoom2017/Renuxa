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
