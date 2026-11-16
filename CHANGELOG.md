# Changelog

All notable changes to Xpu Command. Downloads are on the [releases page](https://github.com/XPU-LAB/xpu-command/releases).

## Unreleased

### Added
- **Web pages.** In Safari, Chrome, Arc, Brave, Edge and Vivaldi, Control my Mac reads the page itself, clicks links and buttons, fills fields and menus, and submits forms. Clicks also reach buttons that only react to a real mouse press. You allow it once in the browser: Develop › Allow JavaScript from Apple Events in Safari, or View › Developer › Allow JavaScript from Apple Events in Chrome-based browsers.
- **"This".** Each request carries the app in front and, for apps the assistant may see, the window title, the page address and your selected text, so "reply to this" or "translate what I selected" works. Activity shows what was shared, and **Include what's in front** in Apps turns it off.
- **More of the mouse.** Right-click (the menu's items can then be chosen), double-click, hover, and drag, such as a file onto a folder or into another app.
- **Clipboard.** The assistant can copy text to paste with ⌘V (undo puts back what you had) and read what you copied when you mention it. Items that password managers mark as secret are never read.
- **Wait.** The assistant waits for a page to load or for text to appear before going on, instead of acting too early.

### Changed
- The spoken answer stays in the language you spoke, even when the request was to translate.

### Safety
- Every new action goes through the same App access gate. Your selected text and anything read from a page or the clipboard count as read, so sending anything out or acting in another app afterwards needs your yes. Submitting a form whose button says Send, Pay, Delete and the like needs your yes. Password, card-number and one-time-code fields are never read or filled.

## 0.7.0 (2026-11-16)

### Added
- **8 languages.** Settings in English, فارسی, Español, Deutsch, Français, العربية, 中文 and 日本語, following your Mac's language or set in Talk › App language. Persian and Arabic mirror the whole window.
- **Voice in 8 languages.** Answer with yes, no or always, say stop, new task, that's all or undo, and use instant commands (open an app, volume, music, mute) in each language. Speech recognition adds Arabic, Chinese and Japanese, and spoken answers use a voice that matches their language.
- **Try it.** After setup, a short tutorial: say "Open Calculator", "Undo that", then "Remind me to stretch in 10 minutes". Each step ticks itself off.
- **Health.** One page that checks everything Xpu Command needs, with a fix for each problem. Crash reports are saved on your Mac and filed only if you choose. **Send diagnostics** saves the logs to your Desktop.
- **Accessibility.** Full keyboard control, VoiceOver labels and announcements, a Text size setting, and support for Reduce Motion and Increase Contrast.
- **Timing in Activity** for each request.

### Changed
- **Sends sooner** after a complete command: 0.7 s of quiet instead of 1.5 s.
- **Clearer errors** that say how to fix the problem.
- **License:** Xpu Command is now open source under the Apache License 2.0, with a privacy policy and terms.

## 0.6.1 (2026-11-16)

### Added
- **Instant commands.** Simple one-step requests run at once without the AI, through the same App access and cancel checks: open or quit an app, open a website, set or mute the volume, pause, play or skip music, dark mode, and web search, in English or Persian («اسپاتیفای رو باز کن»، «صدا رو بذار روی ۳۰»). "Open Calculator" now takes about 0.15 s instead of 4 s. Longer requests and names it doesn't recognize still go to Claude. Turn it off in Mode › Instant commands.

## 0.6.0 (2026-11-16)

### Added
- **Your words appear while you talk.** The pill shows what you're saying as you say it, transcribed on your Mac about once a second. The final text still comes from the whole recording. Turn it off in Talk › More options.
- **The menu-bar icon turns amber** while it's listening or waiting for your answer.

### Changed
- **New look (Signal).** Xpu violet stays the brand color; amber now means "your turn": listening, Allow? questions and downloads. Green means done.
- **New fonts**, bundled in the app: Sora for headings, Manrope for text, JetBrains Mono for paths and times.
- **The pill** glows amber with an amber waveform while listening, spins violet while working, and turns green when done.

## 0.5.2 (2026-11-15)

### Changed
- **The speech model step explains itself**: Whisper turns your voice into text on your Mac, so audio never leaves it; it's a one-time download from Hugging Face (whisper.cpp), checked against its published fingerprint.
- **A progress bar while it downloads**, with the percentage and megabytes, right in Get started.

## 0.5.1 (2026-11-15)

### Fixed
- **Get started never finished when Xpu Command was opened from the download.** macOS runs a temporary copy that can't keep permissions. Xpu Command now asks at launch to move itself to Applications, and Get started shows only that step until it's done.
- **Move and reopen closed the app without reopening it**: the new copy started while the old one was still running. It now waits for the old one to quit.
- **Settings buttons did nothing** in installed builds since 0.3.0: microphone, move to Applications, undo, memory and routines weren't on the window's allow-list, so the page stopped setting up halfway and never noticed permission changes. They're all allowed now, and a test guards against it.
- A log is kept in `~/Library/Logs/Xpu Command/` so problems can be traced.

## 0.5.0 (2026-11-15)

### Changed
- **A simpler Settings window.** A sidebar shows one page at a time: Get started, Talk, Speech, Mode, Apps, Activity, Memory and Routines. Get started shows how many steps are left and disappears when you're ready.
- Settings are grouped rows with on/off switches, every explanation is one short line, and rarely used options sit under **More options**.
- Shorter names: the modes are **Type**, **Agent** and **Control Mac**; speech models show their size and whether they're ready.
- Light and dark themes, with the status (Ready, or steps left) and "Saved" in the sidebar.

## 0.4.1 (2026-11-15)

### Fixed
- **Permissions didn't stick** when the app was opened straight from the download or the DMG: macOS ran a temporary copy and forgot them on every launch, so Get started kept asking. It now offers **Move and reopen**, which moves Xpu Command to Applications (replacing an older copy) and opens it from there.
- **Accessibility or microphone stayed off after an update**, even with the switch on in System Settings. The buttons now clear the old entry, so macOS asks again and the switch works.

## 0.4.0 (2026-11-15)

### Added
- **Get started checklist.** A new install shows three steps with one button each: allow Accessibility, allow the microphone, download the speech model. It also says how to try it, and disappears when everything is ready.
- **Microphone check.** Settings knows whether the microphone is allowed, asks for it, or opens the right System Settings page if it was turned off.
- **Section links** at the top of Settings (Talk, Speech, Words go to, App access, Activity, Memory, Routines).

### Changed
- **Settings save themselves.** There is no Save button any more; every change is saved within half a second, and "Saved" shows in the header.
- **The pill shows whole messages.** Long answers and errors get a third line, "Allow?" questions keep the end of a long command visible, and each state has its own icon: done, problem, allow, about to.
- **Clearer microphone error** that says where to turn it on.

### Fixed
- Text fields (Find an app, Memory, routine names) were white in dark mode.
- Card numbers skipped when a card was hidden.

## 0.3.0 (2026-11-15)

### Added
- **Undo.** Say "undo that" (or press Undo in Activity) within 30 minutes to reverse the last request: apps it opened close, apps it quit reopen, reminders, events and notes it made are deleted, and volume, dark mode and music go back.
- **Memory.** "Remember that Sara is my accountant", or add notes in Settings › Memory. Up to 200 notes, stored only on your Mac, given to the agent as facts, never instructions. "Forget Sara" removes them.
- **Routines.** "Save that as a routine called start my day"; saying the name then runs up to 10 actions at once, without the AI, through the same App access and cancel checks. Requests repeated on different days are offered as routines.
- **Persian.** Approvals understand «آره»، «نه»، «همیشه»; «بس کن»، «کار جدید»، «کافیه» and «برگردون» work as commands. Choosing Persian switches to a multilingual model, and the agent replies in the language you spoke.
- **Custom agents.** Any command-line agent can be used: it reads the request on stdin and answers in plain text or JSON lines. Shells and script interpreters are refused.

### Changed
- AirPods is one switch: press once to talk. Double and triple press keep skipping tracks.

## 0.2.0 (2026-11-14)

### Added
- **AirPods press to talk.** The chosen press starts listening and answers "Allow?" questions; other presses keep controlling Spotify or Music.
- **Voice approvals for Claude Code**: every command is asked out loud; answer "yes", "no" or "always". Commands that delete, push or run downloaded code need a spoken yes.
- **Conversation mode**: after an answer is read aloud, it listens briefly for your reply.

### Changed
- Listening sends when you stop talking, trims silence, drops phantom phrases and plays soft sound cues.
- Double knock ignores taps right after typing or clicking.

## 0.1.0 (2026-11-14)

First public release for macOS (Apple silicon).

- Talk with a mouse button, a keyboard shortcut or a double knock on a MacBook.
- On-device speech to text with verified Whisper models, or OpenAI in the cloud.
- Type what you say into any app.
- Send it to Claude Code or Codex on a project, with live status, a spoken summary and follow-ups.
- Control my Mac: apps, music, volume, reminders, events, notes and email drafts, plus reading and controlling the apps you allow, with an activity log.
