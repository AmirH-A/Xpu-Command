import { listen } from '@tauri-apps/api/event';

type Phase = 'listening' | 'transcribing' | 'typing' | 'working' | 'done' | 'error' | 'pending' | 'confirm' | 'hidden';
type OverlayState = { phase: Phase; text: string; agent: string | null };

const LABEL: Record<Phase, string> = {
  listening: 'Listening', transcribing: 'Transcribing', typing: 'Typing', working: 'Working', done: 'Done', error: 'Problem', pending: 'About to', confirm: 'Allow?', hidden: '',
};

const MIC = '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3" fill="none"/>';
const ICON: Partial<Record<Phase, string>> = {
  done: '<path d="M5 12.5l4.5 4.5L19 7.5" fill="none"/>',
  error: '<path d="M12 6v8" fill="none"/><circle cx="12" cy="18.5" r="1.3" stroke="none"/>',
  confirm: '<path d="M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14" fill="none"/><circle cx="12" cy="18.5" r="1.3" stroke="none"/>',
  pending: '<circle cx="12" cy="12" r="8" fill="none"/><path d="M12 7.5V12l3 2" fill="none"/>',
  typing: '<path d="M4 8h16M4 12h16M4 16h10" fill="none"/>',
};

function fit(phase: Phase, t: string): string {
  if (phase === 'confirm' && t.length > 110) return `…${t.slice(t.length - 108)}`;
  return t;
}

const orb = document.getElementById('orb')!;
const pill = document.getElementById('pill')!;
const phase = document.getElementById('phase')!;
const text = document.getElementById('text')!;
const agent = document.getElementById('agent')!;
const bars = document.getElementById('bars')!;
const N = 32;
for (let i = 0; i < N; i++) bars.appendChild(document.createElement('i'));
const levels: number[] = Array(N).fill(0);

void listen<OverlayState>('overlay', ({ payload }) => {
  pill.dataset.phase = payload.phase;
  phase.textContent = LABEL[payload.phase] ?? '';
  orb.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICON[payload.phase] ?? MIC}</svg>`;
  const shown = fit(payload.phase, payload.text);
  text.textContent = shown;
  text.classList.toggle('long', shown.length > 80);
  text.classList.toggle('live', payload.phase === 'listening' && !payload.text.startsWith('Listening'));
  text.title = payload.text;
  agent.textContent = payload.agent ?? '';
  if (payload.phase === 'listening') levels.fill(0);
});

void listen<number>('level', ({ payload }) => {
  levels.shift();
  levels.push(Math.max(0, Math.min(1, payload)));
  bars.querySelectorAll('i').forEach((el, i) => ((el as HTMLElement).style.height = `${3 + levels[i] * 15}px`));
});
