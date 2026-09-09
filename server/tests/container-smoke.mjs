import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

// Uses only disposable containers and volumes, never the default Compose project.
const image = process.env.VERIFY_IMAGE ?? 'renuxa-server:wechat-verification';
const prefix = `renuxa-smoke-${process.pid}`;
const names = { db: `${prefix}-db`, api: `${prefix}-api`, gateway: `${prefix}-gateway`, worker: `${prefix}-worker` };
const network = `${prefix}-net`;
const volume = `${prefix}-accounts`;
const containers = [];
const token = 'container-verification-token-32-characters';
const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8', timeout: 60000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
const run = (name, args) => {
  docker('run', '-d', '--name', name, '--network', network, ...args);
  containers.push(name);
};
async function eventually(check) {
  let last;
  for (let i = 0; i < 60; i++) {
    try { return await check(); } catch (error) { last = error; await delay(500); }
  }
  throw last;
}
let base;
let jwt;
async function api(path, body, method = 'POST', auth = jwt) {
  const response = await fetch(`${base}/api${path}`, { method, headers: { 'content-type': 'application/json', authorization: `Bearer ${auth}` }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(10000) });
  assert.ok(response.ok, `${path}: HTTP ${response.status} ${response.ok ? '' : await response.text()}`);
  return response;
}
const message = (id, text) => ({ channel: 'wechat:fixture', session_id: 'wechat:fixture:dm:sender', user_id: 'sender', meta: { message_id: id, account_id: 'fixture', is_group: false }, input: [{ role: 'user', content: [{ type: 'text', text }] }] });
try {
  docker('network', 'create', network);
  docker('volume', 'create', volume);
  run(names.db, ['-e', 'POSTGRES_PASSWORD=verification-only', 'postgres:17-alpine']);
  await eventually(() => docker('exec', names.db, 'pg_isready', '-U', 'postgres'));
  const env = ['-e', `DATABASE_URL=postgres://postgres:verification-only@${names.db}:5432/postgres`, '-e', 'JWT_SECRET=container-verification-secret'];
  run(names.api, ['-p', '127.0.0.1:55440:8080', ...env, '-e', 'WECHAT_ENABLED=true', '-e', `WECHAT_GATEWAY_TOKEN=${token}`, image]);
  const address = docker('port', names.api, '8080/tcp');
  base = `http://${address}`;
  await eventually(async () => assert.equal((await fetch(`${base}/health`)).status, 200));
  const auth = await (await api('/auth/register', { email: 'container@test.invalid', password: 'test-password-only' }, 'POST', '')).json();
  jwt = auth.access_token;
  assert.match(auth.user_id, /^[0-9a-f-]{36}$/);
  const binding = await (await api('/integrations/wechat/binding-code', { timezone: 'Asia/Shanghai' })).json();
  assert.match(await (await api('/integrations/wechat/process', message('bind', binding.code), 'POST', token)).text(), /绑定成功/);
  const fields = { name: 'Container fixture', amount: '19.99', currency: 'USD', cadence_unit: 'month', cadence_interval: 1, next_billing_date: '2027-01-31' };
  const sql = `INSERT INTO wechat_drafts(binding_id,fields,version,preview_version) SELECT id,'${JSON.stringify(fields)}'::jsonb,1,1 FROM wechat_bindings WHERE user_id='${auth.user_id}';`;
  docker('exec', names.db, 'psql', '-U', 'postgres', '-v', 'ON_ERROR_STOP=1', '-c', sql);
  docker('restart', names.api);
  await eventually(async () => assert.equal((await fetch(`${base}/health`)).status, 200));
  const committed = await api('/integrations/wechat/process', message('confirm', '确认'), 'POST', token);
  // Discard the first response after headers, then retry the same platform message.
  await committed.body.cancel();
  docker('restart', names.api);
  await eventually(async () => assert.equal((await fetch(`${base}/health`)).status, 200));
  assert.match(await (await api('/integrations/wechat/process', message('confirm', '确认'), 'POST', token)).text(), /已添加订阅/);
  const subscriptions = await (await api('/subscriptions', undefined, 'GET')).json();
  assert.equal(subscriptions.length, 1);
  assert.equal(subscriptions[0].name, fields.name);
  console.log('PASS: migrations, binding, draft across API restart, response interruption and idempotent replay');

  run(names.gateway, ['-v', `${volume}:/var/lib/renuxa-wechat`, '-e', `WECHAT_GATEWAY_TOKEN=${token}`, image, 'im-channel-gateway', '--config', '/etc/renuxa/wechat.toml', 'run']);
  await eventually(() => assert.match(docker('exec', names.gateway, 'curl', '--fail', '--silent', 'http://127.0.0.1:18765/health'), /ok/));
  // Seed a disabled synthetic account. No real WeChat login or external polling occurs.
  docker('stop', names.gateway);
  const fixture = JSON.stringify({ accounts: { fixture: { bot_user_id: 'fixture-only', base_url: '', status: 'disabled', created_at: 1 } } });
  docker('run', '--rm', '-v', `${volume}:/var/lib/renuxa-wechat`, image, 'sh', '-c', 'mkdir -p /var/lib/renuxa-wechat/wechat/fixture && printf %s "$1" > /var/lib/renuxa-wechat/wechat_accounts.json && printf fixture-token > /var/lib/renuxa-wechat/wechat/fixture/bot_token && printf fixture-cursor > /var/lib/renuxa-wechat/wechat/fixture/cursor', 'sh', fixture);
  docker('start', names.gateway);
  await eventually(() => assert.match(docker('exec', names.gateway, 'curl', '--fail', '--silent', 'http://127.0.0.1:18765/api/channels/wechat/accounts'), /fixture-only/));
  docker('restart', names.gateway);
  await eventually(() => assert.match(docker('exec', names.gateway, 'curl', '--fail', '--silent', 'http://127.0.0.1:18765/api/channels/wechat/accounts'), /fixture-only/));
  assert.equal(docker('exec', names.gateway, 'cat', '/var/lib/renuxa-wechat/wechat/fixture/bot_token'), 'fixture-token');
  assert.equal(docker('exec', names.gateway, 'cat', '/var/lib/renuxa-wechat/wechat/fixture/cursor'), 'fixture-cursor');
  assert.throws(() => docker('run', '--rm', '-v', `${volume}:/var/lib/renuxa-wechat`, image, 'im-channel-gateway', '--config', '/etc/renuxa/wechat.toml', 'run'), /another gateway/);
  assert.equal(docker('port', names.gateway), '');
  docker('stop', names.gateway);
  assert.equal((await (await api('/subscriptions', undefined, 'GET')).json()).length, 1);
  console.log('PASS: gateway starts, account/token/cursor survive restart, single-instance lock, no published admin port, Web API works with gateway stopped');
  run(names.worker, [...env, image, 'renuxa-worker']);
  await delay(2000);
  assert.equal(docker('inspect', '--format', '{{.State.Running}}', names.worker), 'true');
  assert.doesNotMatch(docker('logs', names.worker), /worker cycle failed/);
  console.log('PASS: Worker starts from the same image');
} catch (error) {
  for (const name of containers) {
    try { console.error(`${name}: ${docker('logs', '--tail', '15', name)}`); } catch { /* Best-effort diagnostics for test fixtures. */ }
  }
  throw error;
} finally {
  for (const name of containers.reverse()) {
    try { docker('rm', '-f', name); } catch { /* Preserve the original failure. */ }
  }
  try { docker('volume', 'rm', volume); } catch { /* May not exist after setup failure. */ }
  try { docker('network', 'rm', network); } catch { /* May not exist after setup failure. */ }
}
