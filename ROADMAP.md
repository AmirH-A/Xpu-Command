# Roadmap

Where Xpu Command is going: from a voice button to the way you get things done on your Mac, safely, out loud and on your terms. These are plans, not promises; the order follows what people ask for most. Suggest something in [Issues](https://github.com/XPU-LAB/xpu-command/issues).

The same rules hold for everything below: every app and your files start Off, risky actions wait for your yes, changes can be undone, and speech stays on your Mac.

## Now (built)

| | | |
|---|---|---|
| **Works inside web pages** | Reads the page, clicks, fills in and submits forms in Safari and Chrome-based browsers | 0.8 |
| **Understands "this"** | Your selected text and the page you're on come with your request | 0.8 |
| **Right-click, drag, copy and wait** | Uses the mouse and clipboard like you do, and waits for pages to load | 0.8 |
| **Your files** | Find (Spotlight), read (text, PDF, Word), open, move, rename, Trash and new folders in your home folder; all undoable; never hidden folders, Library, apps or scripts | 0.9 |
| **Polished dictation** | Removes fillers, fixes punctuation and fits the app you're in (chat, email, code); "make that shorter", "more polite", "in Spanish", "scratch that" edit what you just said | 0.9 |
| **Your words, snippets and History** | A personal dictionary and replacements the speech model listens for, fixes it learns from your corrections, voice snippets, and every dictation kept on your Mac | next |
| **Edit any text by voice** | Select text in any app and say "make this more formal", "shorter" or "in Spanish" | next |
| **Faster, better speech** | Large v3 Turbo for every language; short requests transcribed 3-4x faster | next |
| **Shows its plan and checks its work** | Multi-step requests show their steps in the pill; each action in a window or page is checked and retried another way once; the model is chosen automatically | 0.9 |
| **"Hey Xpu"** | An optional wake phrase detected on your Mac, so no button is needed | 0.9 |
| **Every message in 8 languages** | Pill hints, steps, results, errors and spoken questions | 0.9 |

## Next (planned)

1. **Signed by Apple, updates itself.** Developer ID signing, notarization and in-app updates: no security warning on install, and Accessibility stays allowed across updates. This is the biggest source of friction today.
2. **Offline Persian voice.** Persian answers spoken by an on-device voice (macOS has none today, so they are shown, not spoken).

## Later (exploring)

- **Your Mac as a tool for any agent.** Xpu Command's gated Mac tools offered as an MCP server to Claude Code, Cursor and Codex, with the same App access rules and confirmations.
- **Choose your model.** Local models (Ollama, Apple on-device) or the cloud, per task, with a fully offline mode.
- **Teach it by doing.** Show a task once and it saves the routine; run routines on a schedule or when something happens.
- **Meeting notes.** Live notes from a call, with action items sent to Reminders.
- **Beyond the Mac.** iPhone and Apple Watch as a remote, and Windows.
