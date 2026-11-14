import { listen } from '@tauri-apps/api/event';

type Phase = 'listening' | 'transcribing' | 'typing' | 'working' | 'done' | 'error' | 'pending' | 'confirm' | 'hidden';
type OverlayState = { phase: Phase; text: string; agent: string | null };

const LABEL: Record<Phase, string> = {
  listening: 'Listening', transcribing: 'Transcribing', typing: 'Typing', working: 'Working', done: 'Done', error: 'Problem', pending: 'About to', confirm: 'Allow?', hidden: '',
};

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
  text.textContent = payload.text;
  agent.textContent = payload.agent ?? '';
  if (payload.phase === 'listening') levels.fill(0);
});

void listen<number>('level', ({ payload }) => {
  levels.shift();
  levels.push(Math.max(0, Math.min(1, payload)));
  bars.querySelectorAll('i').forEach((el, i) => ((el as HTMLElement).style.height = `${3 + levels[i] * 15}px`));
});
