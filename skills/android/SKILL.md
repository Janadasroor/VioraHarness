---
name: android
description: Android development loop (Gradle builds, device tools)
triggers: [android, adb, gradle, emulator, apk, logcat, kotlin]
---
# Android Skill
Loop: edit → build with the project's Gradle wrapper via bash (`background:true` for long builds, follow the task log) → verify via logs/tests.

- Device tools (`adb_*`, emulator control) appear as first-class tools once registered; until then drive devices with `adb` through bash (Ask-gated for install/shell).
- Never run destructive device actions blind — confirm target device (`adb devices`) first.
- Kill background Gradle/logcat tasks via `/tasks` when done.
