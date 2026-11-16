import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

type Settings = {
  button: 'back' | 'forward' | 'middle'; hotkey: 'ctrl_alt_space' | 'alt_shift_space' | 'ctrl_shift_space' | null; mode: 'hold' | 'toggle'; knock: boolean; auto_stop: boolean; sound_cues: boolean; live_words: boolean; airpods: boolean; knock_sensitivity: 'low' | 'medium' | 'high'; stt: 'local' | 'cloud'; local_model: string; language: string;
  delivery: 'type_into_focused' | 'agent' | 'control_mac'; agent: 'claude' | 'codex' | 'custom'; custom_agent: string | null; project_dir: string | null; safety: 'read_only' | 'edit' | 'ask_commands'; conversation: boolean; memory: boolean;
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

function bindCheck(key: 'auto_submit' | 'speak_summary' | 'knock' | 'allow_all_apps' | 'step_by_step' | 'auto_stop' | 'sound_cues' | 'conversation' | 'memory' | 'live_words') {
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
  $('custom-box').hidden = s.agent !== 'custom';
  $('custom-path').textContent = s.custom_agent ?? 'No program chosen';
  $('mac-box').hidden = s.delivery !== 'control_mac';
  $('claude-box').hidden = s.delivery === 'type_into_focused';
  $('profile-row').hidden = s.delivery === 'agent' && s.agent !== 'claude';
  $('knock-row').hidden = !s.knock;
  scheduleSave();
  $('airpods-note').hidden = !s.airpods;
  $('fa-note').hidden = s.language !== 'fa';
  $('ask-note').hidden = s.safety !== 'ask_commands';
  $('access-card').hidden = s.delivery !== 'control_mac';
  $('allow-all-warn').hidden = !s.allow_all_apps;
  renderApps();
  $('project').textContent = s.project_dir ?? 'No project folder chosen';
  const m = models.find((x) => x.id === s.local_model);
  const size = m?.label.match(/\((\d+ MB)/)?.[1] ?? '';
  $('model-state').textContent = `${size ? `${size} · ` : ''}${m?.downloaded ? 'ready' : 'not downloaded'}`;
  $('download-row').hidden = !!m?.downloaded;
  number();
  renderSetup();
}

let savedJson = '';
let saveTimer: number | undefined;

function scheduleSave() {
  if (!s || !savedJson || JSON.stringify(s) === savedJson) return;
  window.clearTimeout(saveTimer);
  flash('Saving…');
  saveTimer = window.setTimeout(async () => {
    try {
      s = await invoke<Settings>('save_settings', { new: s });
      savedJson = JSON.stringify(s);
      flash('Saved');
      render();
    } catch (e) { flash(String(e), true); }
  }, 400);
}

let page = '';

function showPage(id: string) {
  page = id;
  document.querySelectorAll<HTMLElement>('main > .page').forEach((p) => p.classList.toggle('shown', p.id === id));
  document.querySelectorAll<HTMLElement>('#pages a').forEach((a) => a.classList.toggle('on', a.dataset.page === id));
  document.querySelector('main')?.scrollTo(0, 0);
}

function number() {
  document.querySelectorAll<HTMLElement>('#pages a').forEach((a) => {
    a.hidden = $(a.dataset.page!).hidden;
  });
  if (page && $(page).hidden) showPage('talk-card');
}

let downloadPct: number | null = null;

function progressBar(sizeMb: number): HTMLElement {
  const box = document.createElement('div');
  box.className = 'dl';
  const bar = document.createElement('div');
  bar.className = 'progress';
  const fill = document.createElement('i');
  fill.className = 'dl-fill';
  bar.append(fill);
  const label = document.createElement('small');
  label.className = 'dl-label';
  label.dataset.size = String(sizeMb);
  box.append(bar, label);
  return box;
}

function syncProgress() {
  const pct = downloadPct ?? 0;
  document.querySelectorAll<HTMLElement>('.dl-fill').forEach((el) => (el.style.width = `${pct}%`));
  document.querySelectorAll<HTMLElement>('.dl-label').forEach((el) => {
    const size = Number(el.dataset.size || 0);
    el.textContent = pct >= 100 ? 'Checking the fingerprint…' : size ? `${pct}% · ${Math.round((size * pct) / 100)} of ${size} MB` : `${pct}%`;
  });
  $('progress').style.width = `${pct}%`;
}

type Perms = { accessibility: boolean; accessibility_required: boolean; screen_recording: boolean; microphone: 'allowed' | 'not_asked' | 'denied'; needs_move: boolean };
let perms: Perms | null = null;
let hasKey = false;
const BUTTON_NAME: Record<Settings['button'], string> = { forward: 'the forward side button', back: 'the back side button', middle: 'the wheel' };

function renderSetup() {
  if (!perms) return;
  const p = perms;
  const model = models.find((x) => x.id === s.local_model);
  const speechReady = s.stt === 'cloud' ? hasKey : !!model?.downloaded;
  const move = p.needs_move ? [{
    done: false, title: 'Move to Applications', why: "It's running from the download, so macOS keeps forgetting its permissions.",
    label: 'Move and reopen', act: async () => { flash('Moving to Applications…'); await invoke('move_to_applications'); },
  }] : [];
  const sizeMb = Number(model?.label.match(/\((\d+) MB/)?.[1] ?? 0);
  const all: { done: boolean; title: string; why: string; label: string; act: () => void; download?: boolean }[] = [
    {
      done: !p.accessibility_required || p.accessibility, title: 'Allow Accessibility',
      why: 'So the talk button works. Already on? Press anyway, then switch it on again.',
      label: 'Open settings', act: () => invoke('open_permission_settings', { which: 'accessibility' }),
    },
    {
      done: p.microphone === 'allowed', title: 'Allow the microphone', why: p.microphone === 'denied' ? 'It is off. Press Allow, then Allow again when macOS asks.' : 'So Xpu Command can hear you.',
      label: 'Allow',
      act: async () => { await invoke('request_microphone'); await refreshStatus(); },
    },
    {
      done: speechReady, title: s.stt === 'cloud' ? 'Add your OpenAI key' : 'Download the speech model',
      why: s.stt === 'cloud'
        ? 'Under Speech to text.'
        : `Whisper turns your voice into text right on this Mac, so your audio never leaves it. One download${sizeMb ? ` of ${sizeMb} MB` : ''} from Hugging Face (whisper.cpp), checked against its published fingerprint.`,
      label: s.stt === 'cloud' ? 'Show me' : 'Download',
      act: () => { if (s.stt === 'cloud') showPage('speech-card'); else $('download').click(); },
      download: s.stt !== 'cloud',
    },
  ];
  const steps: typeof all = p.needs_move ? move : all;
  const left = p.needs_move ? all.filter((x) => !x.done).length + 1 : steps.filter((x) => !x.done).length;
  $('setup-card').hidden = left === 0;
  $('setup-count').textContent = left ? String(left) : '';
  if (!page) showPage(left ? 'setup-card' : 'talk-card');
  number();
  $('steps').replaceChildren(...steps.map((st, i) => {
    const li = document.createElement('li');
    li.className = st.done ? 'done' : '';
    const n = document.createElement('i');
    n.textContent = st.done ? '✓' : String(i + 1);
    const text = document.createElement('div');
    const b = document.createElement('b');
    b.textContent = st.title;
    const small = document.createElement('small');
    small.textContent = st.done ? 'Done' : st.why;
    text.append(b, small);
    li.append(n, text);
    if (!st.done && st.download && downloadPct !== null) {
      text.append(progressBar(sizeMb));
    } else if (!st.done) {
      const btn = document.createElement('button');
      btn.className = 'primary';
      btn.textContent = st.label;
      btn.onclick = () => { Promise.resolve(st.act()).catch((e) => flash(String(e), true)); };
      li.append(btn);
    }
    return li;
  }));
  syncProgress();
  const how = s.mode === 'hold' ? `Hold ${BUTTON_NAME[s.button]} (or press ${s.hotkey ? 'the keyboard shortcut' : 'the button'}), say “hello”, and let go.` : `Click ${BUTTON_NAME[s.button]}, say “hello”, and pause.`;
  $('try-it').textContent = p.needs_move ? 'After it reopens, allow Accessibility and the microphone here.' : `Then try it: ${how}`;
  const st = $('status');
  st.textContent = left === 0 ? 'Ready' : `${left} step${left === 1 ? '' : 's'} to finish`;
  st.className = `status ${left === 0 ? 'ok' : 'bad'}`;
}

async function refreshModels() {
  models = await invoke<ModelRow[]>('model_status');
  const sel = $<HTMLSelectElement>('local_model');
  sel.replaceChildren(...models.map((m) => { const o = document.createElement('option'); o.value = m.id; o.textContent = m.label.split(' (')[0] + (m.downloaded ? '  ✓' : ''); return o; }));
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
  sel.onchange = () => { s.input_device = sel.value || null; render(); };
}

async function refreshProfiles() {
  const names = await invoke<string[]>('claude_profiles');
  const sel = $<HTMLSelectElement>('claude_profile');
  const opt = (value: string, label: string) => { const o = document.createElement('option'); o.value = value; o.textContent = label; return o; };
  const label = (n: string) => (n === '.claude' ? 'personal (~/.claude)' : `${n.replace('.claude-', '')} (~/${n})`);
  sel.replaceChildren(opt('', "Claude's default"), ...names.map((n) => opt(n, label(n))));
  sel.value = s.claude_profile ?? '';
  sel.onchange = () => { s.claude_profile = sel.value || null; render(); };
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

type Fact = { id: string; text: string; added: string };
type Step = { tool: string; args: unknown };
type RoutineRow = { id: string; name: string; steps: Step[]; added: string; labels: string[] };
type Suggestion = { steps: Step[]; labels: string[]; times: number };

async function refreshRoutines() {
  const rows = await invoke<RoutineRow[]>('routines_list');
  const list = $('routine-list');
  if (!rows.length) {
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = 'No routines yet.';
    list.replaceChildren(li);
  } else {
    list.replaceChildren(...rows.map((r) => {
      const li = document.createElement('li');
      const name = document.createElement('b');
      name.textContent = `“${r.name}”`;
      name.dir = 'auto';
      const what = document.createElement('span');
      what.textContent = r.labels.join(' → ');
      what.title = what.textContent;
      const btns = document.createElement('span');
      btns.className = 'buttons';
      const run = document.createElement('button');
      run.textContent = 'Run';
      run.onclick = async () => { try { await invoke('routine_run', { id: r.id }); await refreshActivity(); } catch (e) { flash(String(e), true); } };
      const del = document.createElement('button');
      del.className = 'ghost';
      del.textContent = 'Delete';
      del.onclick = async () => { try { await invoke('routine_remove', { id: r.id }); await refreshRoutines(); } catch (e) { flash(String(e), true); } };
      btns.append(run, del);
      li.append(name, what, btns);
      return li;
    }));
  }
  const sugg = await invoke<Suggestion[]>('routine_suggestions');
  $('suggestions').replaceChildren(...sugg.map((g) => {
    const box = document.createElement('div');
    box.className = 'suggestion';
    const p = document.createElement('p');
    p.className = 'note';
    p.textContent = `You've asked for this ${g.times} times: ${g.labels.join(' → ')}. Save it as a routine?`;
    const row = document.createElement('div');
    row.className = 'row folder';
    const input = document.createElement('input');
    input.placeholder = 'Name to say, e.g. start my day';
    input.maxLength = 40;
    const btns = document.createElement('span');
    btns.className = 'buttons';
    const save = document.createElement('button');
    save.textContent = 'Save';
    save.onclick = async () => {
      try { await invoke('routine_save', { name: input.value, steps: g.steps }); flash('Routine saved'); await refreshRoutines(); } catch (e) { flash(String(e), true); }
    };
    const no = document.createElement('button');
    no.className = 'ghost';
    no.textContent = 'Not now';
    no.onclick = async () => { try { await invoke('routine_dismiss', { steps: g.steps }); await refreshRoutines(); } catch (e) { flash(String(e), true); } };
    btns.append(save, no);
    row.append(input, btns);
    box.append(p, row);
    return box;
  }));
}

async function refreshMemory() {
  const facts = await invoke<Fact[]>('memory_list');
  const list = $('memory-list');
  $<HTMLButtonElement>('memory-clear').disabled = facts.length === 0;
  if (!facts.length) {
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = 'Nothing remembered yet.';
    list.replaceChildren(li);
    return;
  }
  list.replaceChildren(...facts.map((f) => {
    const li = document.createElement('li');
    const at = document.createElement('time');
    at.textContent = f.added.slice(5);
    const what = document.createElement('span');
    what.textContent = f.text;
    what.title = f.text;
    what.dir = 'auto';
    const del = document.createElement('button');
    del.className = 'ghost';
    del.textContent = 'Remove';
    del.onclick = async () => {
      try { await invoke('memory_remove', { id: f.id }); await refreshMemory(); } catch (e) { flash(String(e), true); }
    };
    li.append(at, what, del);
    return li;
  }));
}

async function refreshUndo() {
  const what = await invoke<string[]>('undo_preview');
  const btn = $<HTMLButtonElement>('undo');
  btn.disabled = what.length === 0;
  $('undo-what').textContent = what.length ? `Last request: ${what.join(', ')}` : '';
}

async function quietly(f: () => Promise<void>) {
  try { await f(); } catch (e) { console.warn(e); }
}

async function refreshActivity() {
  await quietly(refreshUndo);
  await quietly(refreshMemory);
  await quietly(refreshRoutines);
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
  const p = await invoke<Perms>('permission_status');
  perms = p;
  hasKey = await invoke<boolean>('has_cloud_key');
  $('screen-state').textContent = p.screen_recording ? 'Allowed' : "Optional, for apps that can't be read";
  $<HTMLButtonElement>('grant-screen').hidden = p.screen_recording;
  renderSetup();
  const knockErr = await invoke<string | null>('knock_status');
  $('knock-state').textContent = knockErr ? `Double knock is not available: ${knockErr}` : '';
  $('knock-state').className = `hint${knockErr ? ' warn' : ''}`;
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
  document.querySelectorAll<HTMLElement>('#pages a').forEach((a) => { a.onclick = () => showPage(a.dataset.page!); });
  s = await invoke<Settings>('get_settings');
  savedJson = JSON.stringify(s);
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
      flash('Switched to a multilingual speech model: download it below.');
    }
    render();
  };
  const ap = $<HTMLInputElement>('airpods');
  ap.checked = s.airpods;
  ap.onchange = () => { s.airpods = ap.checked; render(); };
  const hk = $<HTMLSelectElement>('hotkey');
  hk.value = s.hotkey ?? '';
  hk.onchange = () => { s.hotkey = (hk.value || null) as Settings['hotkey']; render(); };
  (['button', 'mode', 'knock_sensitivity', 'mac_model', 'local_model', 'agent', 'safety'] as const).forEach(bindSelect);
  bindCheck('auto_submit');
  bindCheck('speak_summary');
  bindCheck('knock');
  bindCheck('auto_stop');
  bindCheck('sound_cues');
  bindCheck('live_words');
  bindCheck('conversation');
  bindCheck('memory');
  bindCheck('allow_all_apps');
  bindCheck('step_by_step');
  try { appRows = await invoke<AppRow[]>('app_access_list'); } catch { appRows = []; }
  $<HTMLInputElement>('app-filter').oninput = renderApps;
  $('grant-screen').onclick = async () => {
    const ok = await invoke<boolean>('request_screen_recording');
    if (!ok) await invoke('open_permission_settings', { which: 'screen' });
  };
  render();
  setInterval(() => { void quietly(refreshStatus); }, 4000);
  await quietly(refreshStatus);

  const addFact = async () => {
    const input = $<HTMLInputElement>('memory-new');
    if (!input.value.trim()) return;
    try { await invoke('memory_add', { text: input.value }); input.value = ''; await refreshMemory(); } catch (e) { flash(String(e), true); }
  };
  $('memory-add').onclick = addFact;
  $<HTMLInputElement>('memory-new').onkeydown = (e) => { if (e.key === 'Enter') addFact(); };
  $('memory-clear').onclick = async () => {
    if (!confirm('Forget everything in Memory?')) return;
    try { await invoke('memory_clear'); await refreshMemory(); flash('Memory cleared'); } catch (e) { flash(String(e), true); }
  };
  $('undo').onclick = async () => {
    try { flash(await invoke<string>('undo_last')); await refreshActivity(); } catch (e) { flash(String(e), true); }
  };
  $('activity-clear').onclick = async () => {
    try { await invoke('activity_clear'); await refreshActivity(); flash('Activity cleared'); } catch (e) { flash(String(e), true); }
  };
  $('pick-custom').onclick = async () => {
    try {
      const p = await invoke<string | null>('pick_custom_agent');
      if (p) { s.custom_agent = p; render(); }
    } catch (e) { flash(String(e), true); }
  };
  $('pick').onclick = async () => {
    try {
      const p = await invoke<string | null>('pick_project');
      if (p) { s.project_dir = p; render(); }
    } catch (e) { flash(String(e), true); }
  };
  $('download').onclick = async () => {
    const btn = $<HTMLButtonElement>('download');
    if (downloadPct !== null) return;
    btn.disabled = true;
    downloadPct = 0;
    $('progress').parentElement!.hidden = false;
    render();
    try {
      await invoke('download_model', { id: s.local_model });
      flash('Model downloaded and verified');
      await refreshModels();
    } catch (e) { flash(String(e), true); }
    downloadPct = null;
    btn.disabled = false;
    $('progress').parentElement!.hidden = true;
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
  await listen<[string, number]>('model-progress', ({ payload: [, pct] }) => { downloadPct = pct; syncProgress(); });
}

init().catch((e) => flash(String(e), true));
