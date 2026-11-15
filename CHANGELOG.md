# Changelog

All notable changes to Xpu Command. Downloads are on the [releases page](https://github.com/XPU-LAB/xpu-command/releases).

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
