# Security

Xpu Command holds three sensitive powers: it hears the microphone, it can type into other apps, and it can
start coding agents that change files. This document lists what we protect, how, and what is left.

## What we protect

| Asset | Threat |
|---|---|
| What you say | recorded when you didn't mean it; kept, leaked or sent somewhere |
| Your keystrokes and screen | read by us (we never need them) |
| The focused app (often a terminal) | text typed into it that does something you didn't say, e.g. an Enter that runs a command |
| Your project and machine | an agent doing more than you asked: running commands, editing outside the project |
| The cloud API key | read from disk, logs or the UI |
| The app itself | a tampered speech model, a malicious web page or agent output driving the app |

## Controls

**Microphone**
- Records only while the talk button is held (or between two clicks in toggle mode), capped at `max_record_seconds` (default 120).
- Audio stays in memory, is never written to disk, and the buffers are zeroed after transcription (`audio::Captured` zeroizes on drop).
- Taps shorter than 250 ms are discarded unheard.
- **Local transcription is the default**, so audio never leaves the computer. Cloud is opt-in, per setting, over TLS to `api.openai.com` only.

**The talk button**
- The hook reads only the configured mouse button: no keyboard hook, no screen capture.
- Only that button is swallowed. Left and right clicks are never touched, and everything else passes through unchanged.
- **Synthetic clicks are ignored** (`LLMHF_INJECTED` on Windows, a non-zero source PID on macOS), so no other program can switch the microphone on.
- A single-instance lock stops a second copy from installing a second hook.
- The tray menu can pause the button at any time.

**Typing into other apps (Mode A)**
- Every transcript goes through `sanitize::clean_transcript`. That removes all control characters (newline, CR, ESC sequences, BEL, C1), bidi overrides and zero-width characters. Speech can therefore never press Enter or inject a terminal escape sequence.
- **Enter is never pressed unless you turn on "Press Enter after typing"** (off by default, with a warning).
- Text is typed as key events, **not pasted**, so it never passes through the clipboard, where clipboard managers and other apps can read it.
- Typing is refused while macOS **secure input** is active, for example in a password field or a terminal's Secure Keyboard Entry.
- **The overlay never takes focus**, so typed text can only reach the app you were in.
  - macOS shows it with `orderFrontRegardless`, never `makeKeyAndOrderFront`. Windows uses `SW_SHOWNOACTIVATE` plus `WS_EX_NOACTIVATE`.
  - Verified on macOS: the frontmost app doesn't change during a full overlay cycle (`XPU_OVERLAY_DEMO`). An earlier build did steal focus, which is why this check exists.

**Agents (Mode B)**
- The agent binary is resolved to an absolute, canonical path. Relative `PATH` entries are skipped.
- **It is started directly, never through a shell**, with a fixed argument list.
- **The prompt goes in on stdin**, not on the command line, because the command line is visible to every local process.
- It works only in the project folder you picked. That folder is validated: it must be absolute, must exist and must be a directory, and it can't be a filesystem root, your home folder or a system folder.
- Safety levels map to each agent's own sandbox. **There is no "bypass permissions" level:**
  - **Claude Code** runs in `--permission-mode plan` (read-only) or `acceptEdits` (edit), always with `--restricted`. That removes every tool that runs commands or code (Bash, PowerShell, …) and WebFetch, and ignores the project's own settings files, so a cloned repository can't use its settings or hooks to run code. *Verified: asked to run a shell command, the agent reports it has no shell tool.*
  - **Codex** runs with `--sandbox read-only` or `workspace-write`: edits only inside the project, and its sandbox blocks network access.
- Agent output is untrusted. It is parsed as JSON, shortened and cleaned, and shown with `textContent` (never as HTML) or spoken.
- Runs time out after 30 minutes and are killed when the app exits (`kill_on_drop`). Saying "stop" kills the current run.
- **A run ends at its first final event.** Nothing is reported after "done" or "failed", and the agent gets 5 seconds to exit before it's killed.
- **Process starts are serialized** (`agents::SPAWN_LOCK`).
  - On macOS, std marks a child's pipes close-on-exec only *after* creating them, so a process spawned by another thread in that gap inherits them.
  - An inherited stdin means the agent never sees end-of-input and hangs. A test caught this about once in 50 runs; with the lock it held across 80 runs.
- **The Claude profile** (`CLAUDE_CONFIG_DIR`) must be a `~/.claude` or `~/.claude-<name>` folder directly in the home folder; any other path is refused. A remembered conversation belongs to the profile that created it and is never resumed under another account.
- **The keyboard shortcut** uses the OS's global-hotkey registration. The app is told about that one combination only and never sees other keystrokes.
- **Continuing a conversation** passes the agent's own session id back with `--resume`.
  - The id comes from agent output, so it is accepted only if it is made of plain id characters and doesn't start with `-`. It can never turn into an extra command-line flag.
  - A resumed run keeps the same sandbox. For Codex, the sandbox flags go before `exec resume`, which was verified on the real CLI.
- Integration tests run fake agent programs to check that:
  - the prompt arrives on stdin and is absent from argv;
  - the working directory is the project;
  - a timeout or "stop" actually kills the process;
  - a session id that looks like a flag is dropped.

**Secrets**
- The OpenAI key is stored in the macOS Keychain or Windows Credential Manager, never in settings files or logs, and never returned to the UI. In memory it is wrapped in `Zeroizing`.
- Error messages from the cloud API report the HTTP status only, never the response body, and URLs are stripped.

**Speech models**
- There is a fixed catalog. Each model has the SHA-256 and size published by its repository, pinned in `stt/model.rs`.
- Downloads use HTTPS only, including redirects. They are hashed while they stream, size-capped, written to a temp file and renamed into place only after verification.
- The file is verified again on every load, so a tampered model is never loaded.

**The app shell (Tauri)**
- A strict Content Security Policy: no remote scripts, no inline scripts, no `eval`, and IPC as the only connection. Prototypes are frozen.
- Capabilities are per window:
  - The **overlay** can only receive events; it can call no commands.
  - The **settings window** can call only the app's own commands (listed in `permissions/app.toml`). Each one validates its input in Rust.
  - Neither window has filesystem, shell, HTTP or opener access.
- Settings are a JSON file, mode 600 inside a 700 directory. They are written atomically, and unknown fields and invalid values are rejected.
- macOS release builds use the hardened runtime, with a single entitlement: microphone.

**Voice approvals (Claude Code)**
- "Edit files; ask me before commands" runs Claude Code with `--permission-prompt-tool`. Every permission prompt goes to a local approvals server: the same hardening as the Mac tool server, with one `approve` tool, which the model itself may not call.
- Each question is shown and spoken. Answers are a short local recording, transcribed and matched to yes, no or always ("no" words win), or a press. With no answer in 45 s, the action is denied.
- Commands that delete files, run as administrator, run downloaded code, push, publish, erase disks or drop data need a spoken "yes"; a press is refused for them (enforced in the server, tested).
- "Always" covers the exact same action for the rest of that request only. Every answer is in the Activity log, with the command shortened.

**Control my Mac (macOS)**
- The planner is Claude Code with every built-in tool off (`--tools ""`), user and project settings ignored (`--restricted`), only our MCP server (`--strict-mcp-config`) and only its tools allowed. It runs in an empty private folder.
- Our tool server listens on 127.0.0.1, on a random port, only during one request. It requires that request's random token (256-bit, constant-time compare), the exact `Host`, and no `Origin` (a web page can't call it). It refuses other paths, non-JSON, and bodies over 1 MB. The config file holding the token is mode 600 and is deleted after the run; leftovers are removed at startup.
- **15 fixed tools**, each with strict input checks: length limits, no control or bidi characters, http(s)-only links without credentials, real email addresses, sane dates. Apps must be bundles in the Applications folders, and Shortcuts must exist by name.
- **Risk gate in the app, not in the model:**
  - Read and Safe tools run.
  - Change tools (reminder, event, note, draft, quit) are shown and run after 3 s unless the user presses.
  - Confirm tools (Shortcuts) run only on a press within 15 s.
  - Calls are serialized, with at most 8 per request.
- **No tool sends, deletes, buys or runs code.** Mail opens a visible draft (tested: the script contains no "send").
- **Values never become code.** AppleScripts are constants. Values are passed after `--` and read via `argv` (tested with flag-like and code-like values). Only `/usr/bin/open`, `/usr/bin/osascript` and `/usr/bin/shortcuts` are started, without a shell.
- Tool results and names are data for the model, never instructions (system prompt), and the gate holds even if the model is fooled.
- **App access (seeing and controlling windows):**
  - Every app is Off until the user sets See, Control or "Ask each time". An app that is Off can be allowed for a single request, through a pill question that the user answers by pressing.
  - "Allow all apps" covers normal apps only. Terminal-like apps (Terminal, iTerm, Script Editor…) need their own setting. Password managers, Keychain Access, System Settings and Xpu Command itself are never accessible.
  - The app enforces access against the app that really owns the window, resolved live (process → bundle id via its executable). The model's claims are never trusted. Before acting, the executor re-checks that the process is still the one the gate approved.
  - Keystrokes are posted to the target process only (`CGEventPostToPid`). Mouse clicks happen only after checking that the point belongs to the target app. Password fields and macOS secure input are never typed into.
  - These always need the user's press, even in allowed apps:
    - controls named Send, Delete, Pay, Buy, Post, Sign out, Unsubscribe, Transfer…;
    - menu items with such names;
    - ⌘Q, ⌘Return, ⌘⌫.
  - **Untrusted screens:** after any screen has been read in a request, links, searches, drafts, Shortcuts and actions in a *different* app need a press. This stops injected text from moving data elsewhere.
  - A glowing outline marks the window being read or controlled. "Step by step" mode makes every action wait for a press. Requests are capped at 40 actions and 5 minutes.
  - **Screenshots:** a fallback that needs Screen Recording. They are JPEG, at most 1400 px, of the target window only, written to the private folder and deleted right after. The log records that a screen was read, never its contents.
- **Activity log:** mode 600, short summaries only, last 300 entries, clearable.
- Release builds add the `automation.apple-events` entitlement. macOS still asks the user once per controlled app.

**Double knock (experimental, off by default)**
- Reads only the MacBook's accelerometer, and only while the setting is on. Samples stay in memory; the detector keeps a gravity estimate and a few tap times, nothing else.
- It can only *start or stop listening*. A false knock opens the microphone and shows the pill; silent or very short recordings are discarded, and nothing is typed or sent without speech.
- Reports are checked for length and range; anything else is ignored rather than acted on.

## Known gaps (to do)

- **Reading a screen exposes its content to the model** (Claude, via your Claude Code account). Only allow See for apps whose content you're fine sending to Claude.
- **Control my Mac trusts same-user processes as much as macOS does.** While a request runs, another program running as you could read the token file and call the tools. The risk gate still applies, and such a program could already act as you.
- **Double knock uses an undocumented sensor.** Apple doesn't document the MacBook accelerometer (`AppleSPUHIDDriver`); a macOS update may change it. The app then reports double knock as unavailable. Windows has no double knock yet.

- **No outside review yet.** No external security review or code audit has been done.
- **Windows builds have not been run.** The hook logic is unit-tested, but nothing has run on Windows itself.
- **Releases aren't signed yet.** Code signing and notarization still need to be set up (Apple Developer ID, Windows Authenticode). Unsigned builds should not be distributed.
- **The settings file has no extra protection on Windows.** It relies on the per-user `AppData` access rights and has no explicit ACL.
- **Codex has only been checked without a login.** The flags and event format are verified against codex-cli 0.167.2, but a signed-in run is still to do. Re-check after Codex CLI upgrades.
- **Dependency audits:** CI runs `cargo audit` and `npm audit`. No known vulnerabilities as of 2026-11-12. Five maintenance warnings remain, all in Linux-only crates (GTK/glib, zbus) or a build-time macro crate, none of which is compiled into the macOS or Windows app (checked with `cargo tree --target`).
- **The agent has no limit on reading inside the project.** Mode B gives the agent your project. Choose the folder deliberately, and use read-only mode for code you don't trust.

## Reporting

Please report vulnerabilities privately to the maintainers. Don't open a public issue.
