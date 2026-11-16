# Roadmap

Where Xpu Command is going: from a voice button to the way you get things done on your Mac, safely, out loud and on your terms. These are plans, not promises; the order follows what people ask for most. Suggest something in [Issues](https://github.com/XPU-LAB/xpu-command/issues).

The same rules hold for everything below: every app and your files start Off, risky actions wait for your yes, changes can be undone, and speech stays on your Mac.

## Now (built)

| | | |
|---|---|---|
| **Works inside web pages** | Reads the page, clicks, fills in and submits forms in Safari and Chrome-based browsers | 0.8 |
| **Understands "this"** | Your selected text and the page you're on come with your request | 0.8 |
| **Right-click, drag, copy and wait** | Uses the mouse and clipboard like you do, and waits for pages to load | 0.8 |
| **Your files** | Find (Spotlight), read (text, PDF, Word), open, move, rename, Trash and new folders in your home folder; all undoable; never hidden folders, Library, apps or scripts | Next update |

## Next (planned)

1. **Signed by Apple, updates itself.** Developer ID signing, notarization and in-app updates: no security warning on install, and Accessibility stays allowed across updates. This is the biggest source of friction today.
2. **Polished dictation.** Removes fillers, fixes punctuation and formats for the app you're in (email, chat, code); edit by voice: "make that shorter", "more polite", "in Spanish".
3. **Shows its plan and checks its work.** Each step appears in the pill; after acting, it verifies the result and tries another way when something fails. Sonnet is chosen automatically for work inside apps.
4. **"Hey Xpu".** An optional wake phrase detected on your Mac, so no button is needed.
5. **Offline Persian voice**, and every message (pill hints, results, errors) in all 8 languages.

## Later (exploring)

- **Your Mac as a tool for any agent.** Xpu Command's gated Mac tools offered as an MCP server to Claude Code, Cursor and Codex, with the same App access rules and confirmations.
- **Choose your model.** Local models (Ollama, Apple on-device) or the cloud, per task, with a fully offline mode.
- **Teach it by doing.** Show a task once and it saves the routine; run routines on a schedule or when something happens.
- **Meeting notes.** Live notes from a call, with action items sent to Reminders.
- **Beyond the Mac.** iPhone and Apple Watch as a remote, and Windows.
