# Appearance themes

Choose a theme in **Settings > Appearance > Theme**. The change applies immediately
and is saved with the other preferences in `settings.json`. Switching the interface
language updates the displayed theme names; the saved identifier stays the same.

| Identifier | English / Chinese | Presentation |
| --- | --- | --- |
| `default` | Default / 默认 | Original beta player before the PR #182 redesign (`e619794`): separate Play and Pause controls, plain Pitch/Rate fields, and the original Waveform ruler, volume and scroll controls |
| `studio` | Studio / 工作室 | Redesigned player from PR #182, retaining the subsequent overview keyboard fix: animated transport controls, editable values with feedback, status display and overview navigation |

Both themes use the existing charcoal palette. A theme includes its player
presentation as well as its color tokens. Frequency colors and non-tonal peaks are
audio data, so neither theme changes Spectral Peaks analysis or rendering colors.
Playback, transforms, volume, zoom, selection, interface scale and font preferences
remain in the shared application state when the presentation changes.

Settings created before theme selection existed, missing theme values and unknown
identifiers use `default`. Existing `default` selections return to the original
player. Users who prefer the redesigned player can select `studio`; no library
migration is required.

## Implementation and extension

The built-in registry in
[`audiolib-core/src/config.rs`](../crates/audiolib-core/src/config.rs) defines stable
identifiers, bilingual names and player styles. It drives settings validation and
the Appearance list. The desktop adapter in
[`src/theme.rs`](../app/audiolib-desktop/src/theme.rs) applies the selected palette
and presentation to the Slint
[`Theme` global](../app/audiolib-desktop/ui/theme.slint).

Keep existing identifiers and the `default` presentation stable. Add a new registry
entry and the corresponding palette/presenter for a new design instead of replacing
the default. Reuse the application inputs and callbacks so new presenters retain
the same playback and Waveform contracts. Keep theme-specific animation and visual
state inside that presenter, and do not create a second audio controller.

For a theme change, verify registry/index mapping, settings round trips, bilingual
Appearance selection, live switching and representative player states. The
headless `ui_scale_tests` cover these UI behaviors. Setting
`AUDIOLIB_UI_SCALE_SNAPSHOT_DIR` saves synthetic PNGs for visual inspection,
including both themes in both languages and a small scaled window. Use the checks
selected by [AGENTS.md](../AGENTS.md#development-checks-by-change-scope), then the
required packaged startup and local installation workflow for application delivery.
