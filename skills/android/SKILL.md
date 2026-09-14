---
name: android
description: Android development loop (Gradle builds, device tools)
triggers: [android, adb, gradle, emulator, apk, logcat, kotlin]
---
# Android Skill
Loop: `adb_devices` to confirm the target (serial auto-selects with one device) → edit → `gradle` tool (`background:true` for long builds, follow the task log) → `adb_install -r` → verify with `adb_logcat`/`adb_shell`.

- `adb` resolves via `ADB_BIN`/`ANDROID_HOME`/`ANDROID_SDK_ROOT`/`PATH` (no hardcoded SDK path); `adb_shell`/`adb_install` are Ask-gated.
- `emulator list|boot` manages AVDs (boot detaches as a task, kill via `/tasks`); `adb_push`/`adb_pull` move files (`/data/local/tmp` for scratch).
- `adb_logcat` is dump mode; for a live tail run `adb logcat` via bash `background:true`.
- `adb_screenshot` captures the screen as base64 vision — use after taps/launches instead of running blind.
- Physical devices with adb auth need explicit approval or `VIORAHARNESS_SANDBOX=off` (`~/.android` keys stay outside the sandbox).
- Never run destructive device actions blind — confirm target device first. Kill background Gradle/logcat tasks via `/tasks` when done.
