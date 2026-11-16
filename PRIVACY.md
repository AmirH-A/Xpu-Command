# Privacy policy

*Effective 16 November 2026*

Xpu Command is a Mac app that turns your voice into typing, requests to coding agents, and actions on your Mac. It is built to keep your data on your Mac. This page explains exactly what it handles and where it goes.

## What stays on your Mac

- **Your voice.** Audio is recorded only while you hold the talk button, after a knock or AirPods press, or while the app waits for a short reply. With **On this Mac** speech (the default), audio is turned into text on your Mac by Whisper and is never sent anywhere. Audio is kept in memory only and wiped after use; it is never saved to disk.
- **Memory, routines and the activity log.** Saved in the app's settings folder (`~/Library/Application Support/xpu-command/`), readable only by your user account.
- **Settings.** In the same folder. Your OpenAI key, if you add one, is kept in the macOS keychain, never in a file.
- **Logs.** A diagnostic log is kept in `~/Library/Logs/Xpu Command/`. It records what the app did (for example "permissions changed" or "instant command in 150 ms"), not what you said.

## What leaves your Mac, and only when you choose it

- **Cloud speech (optional, off by default).** If you choose **Cloud** speech, your audio is sent to OpenAI with your own API key, under OpenAI's terms and privacy policy.
- **Coding agents and Control my Mac.** Your request text is sent to the agent you chose (Claude Code, Codex or a custom agent), which runs with your own account and is governed by that provider's terms and privacy policy. Notes in Memory are added to those requests as context.
- **Downloads.** Speech models are downloaded from Hugging Face. App updates and releases are downloaded from GitHub. These services see your IP address, as with any download.
- **Diagnostics.** If you press **Send diagnostics** or report a crash, the app prepares a file or a GitHub issue for you to review. Nothing is sent unless you send it yourself.

## What we don't collect

There is no account, no analytics, no tracking and no advertising. XPU Lab does not receive your audio, transcripts, memory, activity or settings.

## Your control

Turn off Memory, clear it, or clear the activity log in Settings. Remove all app data by deleting `~/Library/Application Support/xpu-command/` and `~/Library/Logs/Xpu Command/`. Revoke microphone, Accessibility or automation access any time in System Settings › Privacy & Security.

## Changes and contact

Changes to this policy are recorded in the project's history on GitHub. Questions: open an issue at https://github.com/XPU-LAB/xpu-command/issues.
