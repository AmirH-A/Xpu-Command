import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

type Settings = {
  button: 'back' | 'forward' | 'middle'; hotkey: 'ctrl_alt_space' | 'alt_shift_space' | 'ctrl_shift_space' | null; mode: 'hold' | 'toggle'; knock: boolean; auto_stop: boolean; sound_cues: boolean; airpods: boolean; knock_sensitivity: 'low' | 'medium' | 'high'; stt: 'local' | 'cloud'; local_model: string; language: string;
  delivery: 'type_into_focused' | 'agent' | 'control_mac'; agent: 'claude' | 'codex'; project_dir: string | null; safety: 'read_only' | 'edit' | 'ask_commands'; conversation: boolean;
  speak_summary: boolean; auto_submit: boolean; max_record_seconds: number; input_device: string | null; claude_profile: string | null; mac_model: 'haiku' | 'sonnet'; app_access: Record<string, Access>; allow_all_apps: boolean; step_by_step: boolean;
};
type ModelRow = { id: string; label: string; downloaded: boolean };
type AgentInfo = { kind: 'Claude' | 'Codex' | 'claude' | 'codex'; found: boolean; path: string | null };

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
let s: Settings;
let models: ModelRow[] = [];

function seg(id: string, value: string, onPick: (v: string) => void) {
  const box = $(id);
  box.querySelectorAll('button').forEach((b) => {
    b.classList.toggle('on', b.dataset.v === value);
    b.onclick = () => { onPick(b.dataset.v!); render(); };
  });
}

function bindSelect<K extends keyof Settings>(key: K) {
  const el = $<HTMLSelectElement>(key as string);
  el.value = String(s[key]);
  el.onchange = () => { (s as any)[key] = el.value; render(); };
}

type Access = 'off' | 'see' | 'control' | 'ask';
type AppRow = { bundle: string; name: string; class: 'normal' | 'terminal' | 'excluded' };
let appRows: AppRow[] = [];

function bindCheck(key: 'auto_submit' | 'speak_summary' | 'knock' | 'allow_all_apps' | 'step_by_step' | 'auto_stop' | 'sound_cues' | 'conversation') {
  const el = $<HTMLInputElement>(key);
  el.checked = s[key];
  el.onchange = () => { s[key] = el.checked; render(); };
}

function render() {
  seg('stt', s.stt, (v) => (s.stt = v as Settings['stt']));
  seg('delivery', s.delivery, (v) => (s.delivery = v as Settings['delivery']));
  $('local-box').hidden = s.stt !== 'local';
  $('cloud-box').hidden = s.stt !== 'cloud';
  $('type-box').hidden = s.delivery !== 'type_into_focused';
  $('agent-box').hidden = s.delivery !== 'agent';
  $('mac-box').hidden = s.delivery !== 'control_mac';
  $('claude-box').hidden = s.delivery === 'type_into_focused';
  $('profile-row').hidden = s.delivery === 'agent' && s.agent !== 'claude';
  $('knock-row').hidden = !s.knock;
  $('airpods-note').hidden = !s.airpods;
  $('fa-note').hidden = s.language !== 'fa';
  $('ask-note').hidden = s.safety !== 'ask_commands';
  $('access-card').hidden = s.delivery !== 'control_mac';
  $('allow-all-warn').hidden = !s.allow_all_apps;
  renderApps();
  $('project').textContent = s.project_dir ?? 'No project folder chosen';
  const m = models.find((x) => x.id === s.local_model);
  $('model-state').textContent = m?.downloaded ? 'Downloaded and ready' : 'Not downloaded yet';
  $<HTMLButtonElement>('download').hidden = !!m?.downloaded;
}

async function refreshModels() {
  models = await invoke<ModelRow[]>('model_status');
  const sel = $<HTMLSelectElement>('local_model');
  sel.replaceChildren(...models.map((m) => { const o = document.createElement('option'); o.value = m.id; o.textContent = m.label + (m.downloaded ? '  ✓' : ''); return o; }));
  sel.value = s.local_model;
}

async function refreshDevices() {
  const names = await invoke<string[]>('input_devices');
  const sel = $<HTMLSelectElement>('input_device');
  const opt = (value: string, label: string) => { const o = document.createElement('option'); o.value = value; o.textContent = label; return o; };
  const list = [opt('', 'System default'), ...names.map((n) => opt(n, n))];
  if (s.input_device && !names.includes(s.input_device)) list.push(opt(s.input_device, `${s.input_device} (not connected)`));
  sel.replaceChildren(...list);
  sel.value = s.input_device ?? '';
  sel.onchange = () => { s.input_device = sel.value || null; };
}

async function refreshProfiles() {
  const names = await invoke<string[]>('claude_profiles');
  const sel = $<HTMLSelectElement>('claude_profile');
  const opt = (value: string, label: string) => { const o = document.createElement('option'); o.value = value; o.textContent = label; return o; };
  const label = (n: string) => (n === '.claude' ? 'personal (~/.claude)' : `${n.replace('.claude-', '')} (~/${n})`);
  sel.replaceChildren(opt('', "Claude's default"), ...names.map((n) => opt(n, label(n))));
  sel.value = s.claude_profile ?? '';
  sel.onchange = () => { s.claude_profile = sel.value || null; };
}

type ActivityEntry = { at: string; tool: string; summary: string; outcome: 'done' | 'failed' | 'cancelled' | 'declined' | 'refused'; detail: string };
const OUTCOME: Record<ActivityEntry['outcome'], string> = { done: 'Done', failed: 'Failed', cancelled: 'Cancelled', declined: 'Not allowed', refused: 'Refused' };

const ACCESS_LABEL: Record<Access, string> = { off: 'Off', see: 'See', control: 'Control', ask: 'Ask each time' };

function levelOf(r: AppRow): Access {
  if (r.class === 'excluded') return 'off';
  const set = s.app_access[r.bundle];
  if (set) return set;
  return s.allow_all_apps && r.class === 'normal' ? 'control' : 'off';
}

function renderApps() {
  const list = $('apps');
  if (!list || !appRows.length) return;
  const q = $<HTMLInputElement>('app-filter').value.trim().toLowerCase();
  const rows = appRows
    .filter((r) => !q || r.name.toLowerCase().includes(q) || r.bundle.toLowerCase().includes(q))
    .sort((a, b) => Number(levelOf(b) !== 'off') - Number(levelOf(a) !== 'off') || a.name.localeCompare(b.name));
  const allowed = appRows.filter((r) => levelOf(r) !== 'off').length;
  $('app-count').textContent = `${allowed} of ${appRows.length} apps allowed`;
  list.replaceChildren(...rows.slice(0, 300).map((r) => {
    const li = document.createElement('li');
    const name = document.createElement('span');
    name.textContent = r.name;
    name.title = r.bundle;
    li.append(name);
    if (r.class === 'excluded') {
      const b = document.createElement('b');
      b.textContent = 'Never';
      b.title = 'Holds passwords or security settings, or is Xpu Command itself';
      li.append(b);
      return li;
    }
    if (r.class === 'terminal') {
      const w = document.createElement('small');
      w.textContent = 'can run anything';
      w.className = 'warn';
      name.append(' ', w);
    }
    const sel = document.createElement('select');
    (['off', 'see', 'control', 'ask'] as Access[]).forEach((a) => {
      const o = document.createElement('option');
      o.value = a;
      o.textContent = ACCESS_LABEL[a];
      sel.append(o);
    });
    sel.value = levelOf(r);
    sel.onchange = () => {
      const v = sel.value as Access;
      const inherited = s.allow_all_apps && r.class === 'normal' ? 'control' : 'off';
      if (v === inherited) delete s.app_access[r.bundle];
      else s.app_access[r.bundle] = v;
      renderApps();
    };
    li.append(sel);
    return li;
  }));
}

async function refreshUndo() {
  const what = await invoke<string[]>('undo_preview');
  const btn = $<HTMLButtonElement>('undo');
  btn.disabled = what.length === 0;
  $('undo-what').textContent = what.length ? `Undo would: ${what.join(', ')}` : '';
}

async function refreshActivity() {
  await refreshUndo();
  const rows = await invoke<ActivityEntry[]>('activity_recent');
  const list = $('activity');
  if (!rows.length) {
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = 'Nothing yet.';
    list.replaceChildren(li);
    return;
  }
  list.replaceChildren(...rows.map((r) => {
    const li = document.createElement('li');
    const at = document.createElement('time');
    at.textContent = r.at.slice(5, 16);
    const what = document.createElement('span');
    what.textContent = r.summary || r.tool;
    what.title = r.detail;
    const out = document.createElement('b');
    out.className = r.outcome;
    out.textContent = OUTCOME[r.outcome] ?? r.outcome;
    li.append(at, what, out);
    return li;
  }));
}

async function refreshStatus() {
  const p = await invoke<{ accessibility: boolean; accessibility_required: boolean; screen_recording: boolean }>('permission_status');
  $('screen-state').textContent = p.screen_recording ? 'Screenshots: allowed' : 'Screenshots: not allowed (optional)';
  $<HTMLButtonElement>('grant-screen').hidden = p.screen_recording;
  const ready = !p.accessibility_required || p.accessibility;
  $('perm-banner').hidden = ready;
  const st = $('status');
  st.textContent = ready ? 'Talk button ready' : 'Needs permission';
  st.className = `status ${ready ? 'ok' : 'bad'}`;
  const knockErr = await invoke<string | null>('knock_status');
  $('knock-state').textContent = knockErr ? `Double knock is not available: ${knockErr}` : '';
  $('knock-state').className = `note${knockErr ? ' warn' : ''}`;
  await refreshActivity();
  const agents = await invoke<AgentInfo[]>('agent_status');
  $('agents').replaceChildren(...agents.map((a) => {
    const el = document.createElement('span');
    const name = String(a.kind).toLowerCase() === 'claude' ? 'Claude Code' : 'Codex';
    el.className = a.found ? 'found' : 'missing';
    el.textContent = a.found ? `${name}: installed` : `${name}: not found`;
    if (a.path) el.title = a.path;
    return el;
  }));
  $('key-state').textContent = (await invoke<boolean>('has_cloud_key')) ? 'A key is saved in your keychain.' : 'No key saved.';
}

function flash(msg: string, err = false) {
  const el = $('saved');
  el.textContent = msg;
  el.className = `saved${err ? ' err' : ''}`;
  if (!err) setTimeout(() => (el.textContent === msg ? (el.textContent = '') : null), 2500);
}

async function init() {
  s = await invoke<Settings>('get_settings');
  await refreshModels();
  await refreshDevices();
  await refreshProfiles();
  const lang = $<HTMLSelectElement>('language');
  lang.value = s.language;
  lang.onchange = () => {
    s.language = lang.value;
    if (s.language !== 'en' && s.language !== 'auto' && s.local_model.endsWith('.en')) {
      s.local_model = s.language === 'fa' || s.local_model === 'small.en' ? 'small' : 'base';
      $<HTMLSelectElement>('local_model').value = s.local_model;
      flash('That language needs a multilingual speech model: switched it for you. Download it below, then Save.');
    }
    render();
  };
  const ap = $<HTMLInputElement>('airpods');
  ap.checked = s.airpods;
  ap.onchange = () => { s.airpods = ap.checked; render(); };
  const hk = $<HTMLSelectElement>('hotkey');
  hk.value = s.hotkey ?? '';
  hk.onchange = () => { s.hotkey = (hk.value || null) as Settings['hotkey']; };
  (['button', 'mode', 'knock_sensitivity', 'mac_model', 'local_model', 'agent', 'safety'] as const).forEach(bindSelect);
  bindCheck('auto_submit');
  bindCheck('speak_summary');
  bindCheck('knock');
  bindCheck('auto_stop');
  bindCheck('sound_cues');
  bindCheck('conversation');
  bindCheck('allow_all_apps');
  bindCheck('step_by_step');
  try { appRows = await invoke<AppRow[]>('app_access_list'); } catch { appRows = []; }
  $<HTMLInputElement>('app-filter').oninput = renderApps;
  $('grant-screen').onclick = async () => {
    const ok = await invoke<boolean>('request_screen_recording');
    if (!ok) await invoke('open_permission_settings', { which: 'screen' });
  };
  render();
  await refreshStatus();
  setInterval(refreshStatus, 4000);

  $('undo').onclick = async () => {
    try { flash(await invoke<string>('undo_last')); await refreshActivity(); } catch (e) { flash(String(e), true); }
  };
  $('activity-clear').onclick = async () => {
    try { await invoke('activity_clear'); await refreshActivity(); flash('Activity cleared'); } catch (e) { flash(String(e), true); }
  };
  $('grant-ax').onclick = () => invoke('open_permission_settings', { which: 'accessibility' });
  $('pick').onclick = async () => {
    try {
      const p = await invoke<string | null>('pick_project');
      if (p) { s.project_dir = p; render(); }
    } catch (e) { flash(String(e), true); }
  };
  $('download').onclick = async () => {
    const btn = $<HTMLButtonElement>('download');
    btn.disabled = true;
    (document.querySelector('.progress') as HTMLElement).hidden = false;
    try {
      await invoke('download_model', { id: s.local_model });
      flash('Model downloaded and verified');
      await refreshModels();
    } catch (e) { flash(String(e), true); }
    btn.disabled = false;
    (document.querySelector('.progress') as HTMLElement).hidden = true;
    render();
  };
  $('save-key').onclick = async () => {
    const input = $<HTMLInputElement>('key');
    try { await invoke('set_cloud_key', { key: input.value }); flash('Key saved to your keychain'); }
    catch (e) { flash(String(e), true); }
    input.value = '';
    await refreshStatus();
  };
  $('clear-key').onclick = async () => { await invoke('clear_cloud_key'); flash('Key removed'); await refreshStatus(); };
  $('save').onclick = async () => {
    try { s = await invoke<Settings>('save_settings', { new: s }); flash('Saved'); render(); }
    catch (e) { flash(String(e), true); }
  };
  await listen<[string, number]>('model-progress', ({ payload: [, pct] }) => { $('progress').style.width = `${pct}%`; });
}

init().catch((e) => flash(String(e), true));
