import { currencySymbols } from './constants';

export function money(amount: number, currency: string) {
  return `${currencySymbols[currency] ?? `${currency} `}${amount.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

