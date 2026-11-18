# Changelog

All notable changes to Xpu Command. Downloads are on the [releases page](https://github.com/XPU-LAB/xpu-command/releases).

## 0.10.0 (2026-11-18)

### Added
- **Linux.** Xpu Command runs on Linux (X11 and Wayland) with every feature: dictation fitted to the text around the cursor and editing by voice through AT-SPI, the talk button (an X11 grab, or the mouse itself on Wayland), the keyboard shortcut (the GlobalShortcuts portal on Wayland), local and cloud speech, agents, and "Control my computer" with the same tools, access settings, approvals and undo. Media, volume, dark mode, reminders, events, notes, email drafts, the Trash and shortcuts use their Linux counterparts. Packages: `.deb`, `.rpm` and AppImage. The window says "computer" instead of "Mac" in all 8 languages.
- **Your words.** A personal dictionary in Settings: names, products and jargon steer the speech model toward them and are typed with your spelling and capitals; replacements turn what it writes into what you want ("xerox" → "Zerox"). Your words also reach the assistant and agents in requests. In tests, adding "Hooman, XPU Command, Sara" turned "Huuman" into "Hooman" and "Sarah" into "Sara".
- **It learns your fixes.** When you correct a name or term right after dictating it, the fix is offered in Your words for one click. Nothing is added without you; ordinary words ("their" → "they're") are never suggested.
- **Snippets.** Say a short phrase on its own ("my email", "my sign-off") and its text is typed.
- **History.** Every dictation is kept on your Mac, searchable, with a Copy button, including anything that couldn't be typed (a password field, a missing permission), so nothing you say is lost. Turn it off and the file is deleted.
- **Edit any selected text by voice.** Select text in any app and say "make this more formal", "shorter", "fix the grammar" or "in Spanish". Several paragraphs keep their line breaks.
- **Large v3 Turbo** speech model (574 MB): far better for Persian, Spanish, accents and names, and now the suggested model when you pick Persian.

### Changed
- **Dictation fits the text around the cursor.** Inside a sentence it doesn't start with a capital ("…and then" + "we ship"), adds the missing space, and leaves out a final period when the sentence goes on. Code and terminals are typed as said.
- **Speech to text is 3-4x faster for short requests.** The model reads only as much of its window as you spoke: "Open Slack" went from 1.1 s to 0.3 s with Large v3 Turbo and from 70 ms to 30 ms with Base on an M4, with the same words. Flash attention is on.

### Fixed
- Whisper repeating itself into the silence after you stop ("Open Slack. Open Slack. Open Slack.", or a sentence written twice) is caught and removed.
- Persian half-spaces (می‌روم) and emoji joiners are kept instead of stripped.

## 0.9.2 (2026-11-17)

### Added
- **Health › How it's doing.** Success and typical speed of your last 100 Control my Mac requests, against the goals (90% of requests work; one step in about 3 s; several steps in about 8 s). Each request's steps and times are kept on your Mac only.
- A benchmark of 31 real tasks for developers (`--selftest-bench`), run end to end and checked on the Mac.

### Fixed
- Apps asked to come forward now really do: Xpu Command asks through Accessibility, then LaunchServices, then raises the window, and checks. `open_app` reports honestly when another app stayed in front.
- Finder (and other system apps outside Applications) can be opened and controlled by name.
- An app with no window open (closed, hidden or not running) is opened and its window waited for, instead of failing.
- Buttons that refuse a click (often in Electron apps like Cursor and Slack) are clicked with the mouse, or focused and pressed with Space; when nothing works the assistant is told to use the menu bar or a shortcut.
- When a browser doesn't allow page access yet, reading the page falls back to reading the window, so the request goes on.
- Play, pause, next and previous use the media keys when Xpu Command isn't allowed to control Spotify or Music directly.
- Opening a folder brings Finder to the front.
- File search finds files made moments ago anywhere in your home folder, matches names however they are separated ("bench invoice" finds bench-invoice.pdf), and lists your own documents before build and cache files. In the benchmark, reading a file went from 7.3 s to 3.7 s.

## 0.9.1 (2026-11-17)

### Changed
- **Asks far less.** Naming an app in your request ("reply in Slack", "open Notes and…") allows it for that request without a question; so does "this" or "here" for the app in front, and mentioning a file or folder for Your files. Terminals, password managers and system settings are never allowed this way.
- **"Always".** When the assistant asks for an app, say "always" and it is saved in Settings › Apps, so it doesn't ask again; "yes" or a press allows it for this request, "no" declines.
- After you say yes to acting in an app, the assistant doesn't ask again for that app in the same request. Sending, paying, deleting and sending anything out after reading still need your yes.

## 0.9.0 (2026-11-17)

### Added
- **Your files.** Control my Mac finds files by name (Spotlight, plus files made moments ago), reads documents (text, PDF, Word, RTF, HTML), opens files and folders, moves, renames, makes folders and moves things to the Trash. Every change can be undone; nothing is ever replaced. Files start Off: allow **Your files** at the top of Settings › Apps (See to find and read, Control to change), or say yes when asked. Hidden folders, Library, apps, installers and scripts are never touched, and the Trash always needs your yes.
- [Roadmap](ROADMAP.md), also on the website.
- **Polished dictation.** Type mode removes "um"s and stutters, fixes punctuation spacing and capitals, and fits the app you're in: a chat message loses its final period, a terminal or code editor gets your words as said. A second dictation in the same field gets a space before it. Turn it off with **Polish my dictation** in Mode.
- **Edit by voice.** Right after dictating, say "make that shorter", "more polite", "more formal", "fix the grammar", "in Spanish" or "scratch that" (also in the other seven languages). The text is rewritten by Claude Haiku with no tools and replaced in place; Xpu Command first checks it is still in the field, so nothing else is touched.
- **Shows its plan and checks its work.** For requests with several steps, Control my Mac shows its plan in the pill ("Mac · 2/3") and in Activity. After clicking, typing or filling a form it checks the result, tries one other way if it didn't work, and otherwise says plainly what failed.
- **Automatic model** (new default): Haiku for quick one-step requests, Sonnet for work inside apps, on-screen context and longer requests. Activity shows which was used.
- **"Hey Xpu".** Optional, off by default: say "Hey Xpu", then your request, or both in one breath. Checked on your Mac with the speech model you already have; audio is kept for a few seconds in memory only and never leaves the Mac. Paused while Xpu Command listens, asks or speaks. macOS shows the microphone indicator while it's on.
- **Every message in your language.** The pill (hints, steps, results, errors) and the spoken questions follow the app language in Settings, in all 8 languages, with right-to-left text for Persian and Arabic.

### Fixed
- Searches with many results no longer fail.

## 0.8.0 (2026-11-16)

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
