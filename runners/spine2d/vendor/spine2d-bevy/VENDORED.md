# spine2d-bevy (vendored)

Copy of `spine2d-bevy` 0.5.0 from [Latias94/spine2d](https://github.com/Latias94/spine2d) (tag v0.5.0, commit
`16a3434`), Copyright (c) 2025 Mingzhen Zhuang, licensed MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`, copied from
upstream).

Modified for the benchmark: `SpineTrackEntrySettings::with_track_time(f32)` was added (about 10 lines, marked
`BENCH PATCH`), so the runner can set each instance's initial animation phase. Everything else is unmodified.
