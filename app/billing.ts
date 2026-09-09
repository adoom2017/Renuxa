type BillingSchedule = { cadence: string; nextDate: string; cadenceInterval?: number; anchorDay?: number };

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
