export type View = 'dashboard' | 'subscriptions' | 'bills' | 'notifications' | 'settings';
export type Status = 'active' | 'paused' | 'cancelled';
export type BillStatus = 'estimated' | 'paid' | 'skipped' | 'refunded';
export type Locale = 'zh-CN' | 'en';

export type Subscription = {
  id: string; name: string; plan: string; amount: number; currency: string; cadence: string;
  nextDate: string; category: string; status: Status; color: string; iconUrl?: string; reminderOffsets?: number[]; cadenceInterval?: number; anchorDay?: number;
};

export type Bill = { id: string; subscriptionId?: string; subscription: string; date: string; amount: number; currency: string; status: BillStatus };
export type Notice = { id: string; title: string; body: string; date: string; read: boolean; kind: 'renewal' | 'bill' | 'system' };
export type NotificationSettings = {
  telegram_enabled: boolean; telegram_bot_token_configured: boolean; telegram_chat_id: string;
};

