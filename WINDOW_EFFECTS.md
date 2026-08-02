# Window Effects (fork surface)

Renderer-native window effects for this zed fork: **theme-switch
transitions** (rectangle, circle, or circle-blur reveal) and **modal backdrop blur**
(CSS `backdrop-filter` style). This document describes the fork's diff vs
upstream zed so future upstream syncs stay tractable.

## Why this design

The standalone `pipit-window` crate (in the pipit repo) achieves theme
transitions out-of-process: `PrintWindow` screen capture, an owned popup
overlay, a separate WGPU/DX12 device, and dedicated threads. That was a hack
necessitated by not owning gpui. This fork owns gpui, so both effects are
implemented inside the renderers instead: the renderer already holds the
framebuffer, so a transition is just a retained snapshot texture plus a
full-screen composite pass, synchronized with the render loop. No capture
APIs, no extra windows, no extra threads, no timeouts-by-thread.

pipit-window remains historical context for the easing and reveal geometry;
it is otherwise obsolete once pipit itself migrates to this API. pipit's
existing `paint_backdrop_blur` call keeps working because the `BackdropBlur`
scene primitive is independent from theme transitions and is preserved.

## Fork surface vs upstream

Pre-existing (kept as the blur foundation):

- `6a1bc26041` gpui: add BackdropBlur primitive with a DirectX implementation
- `96cf287ae3` gpui: add BackdropBlur Metal + wgpu implementations

Added by the window-effects work (all additive):

| File | Change |
| --- | --- |
| `crates/gpui/src/window_effects.rs` | **New.** `RevealOrigin`, `TransitionOptions`, `window_effects_enabled()`, easing/reveal math + tests, `ThemeTransitionState` state machine. |
| `crates/gpui/src/scene.rs` | `TransitionParams` (repr(C), GPU-facing), the `Scene::transition` field, and `Scene::clear()` reset. |
| `crates/gpui/src/window.rs` | `Window::{begin,start,cancel}_theme_transition`, progress/timing metrics, state-machine integration in `draw()`, and cancellation on resize or first key/mouse press. |
| `crates/gpui/src/platform.rs` | Four defaulted `PlatformWindow` methods for capability/snapshot capture and starting/stopping the platform animation frame source. Unsupported platforms retain the synchronous theme path. |
| `crates/gpui/src/gpui.rs` | `mod window_effects` + re-export. |
| `crates/gpui_windows/` | DirectX backend: stable old/incoming snapshots, one-time downsampled CircleBlur levels, and composite pipeline (`directx_renderer.rs`); `theme_transition` shaders (`shaders.hlsl`); fxc module entry (`build.rs`); per-window transition registration and fair vsync messages (`platform.rs`, `events.rs`, `window.rs`). |
| `crates/theme_settings/src/theme_settings.rs` | `reload_theme` orchestration (the single theme choke point). |
| `crates/workspace/src/modal_layer.rs` | Backdrop-blur consumer for modals with `fade_out_background`. |
| `crates/theme_settings/Cargo.toml` | `futures` dep for the snapshot wait/timeout race. |

## How the theme transition works

`TransitionOptions` carries one of three distinct `TransitionStyle` values:
`Rectangle`, sharp `Circle`, or `CircleBlur`, plus reveal origin, duration,
and blur radius. Their geometry, timings, and easing mirror
`ui-components/components/motion/theme-toggle.tsx`: rectangle animates the
selected CSS inset over 400ms ease-out; both circle styles animate
`circle(0% -> 150%)` over 700ms with `cubic-bezier(0.4, 0, 0.2, 1)`; only
CircleBlur sharpens the incoming frame from 8px blur to zero.

State machine on `Window`: `Idle → ReadyToStart → Active → Idle`.

1. `reload_theme` computes the configured theme. If it actually changed
   (compared by name + appearance), window effects are enabled, and reduced
   motion is off: every open window whose renderer supports transitions
   synchronously snapshots its last presented frame through
   `begin_theme_transition(cx, TransitionOptions::default())`.
2. Once every window is ready, the host applies the theme once
   (`GlobalTheme::update_theme` + `refresh_windows`) and calls
   `start_theme_transition()` in each captured window.
3. The first active draw is copied to a dedicated incoming-frame texture.
   CircleBlur precomputes full and medium blur levels once at quarter
   resolution; Rectangle and Circle skip blur work entirely. Every subsequent
   draw only updates `TransitionParams` and composites the retained textures.
   On Windows, active transition HWNDs receive a dedicated message from the
   existing vsync thread. A per-window pending flag coalesces those messages,
   so one busy or active window cannot starve another and no extra animation
   thread is created.

Fallbacks (all invisible to the user): unsupported platform / invalid
options / transition already in flight / no stable presented frame →
synchronous swap; window resize or first user input during the animation →
cancel. Every open window animates independently.

## Modal backdrop blur

`ModalLayer` paints, for modals that opt in via `fade_out_background`
(security/trust modal, move-to-applications, disconnected overlay, remote
connection): a `canvas` child calling `window.paint_backdrop_blur(bounds,
Corners::default(), px(24.0), 1.6)`, underneath the existing tinted dim
layer (which must stay a separate child above the blur — a div paints its
`bg` before its children). The primitive re-captures every frame, so the
backdrop stays live; no per-modal state is needed.

## Environment variables

- `ZED_WINDOW_EFFECTS=0` (or `off`/`false`) — disables both effects;
  everything falls back to upstream behavior.
- `GPUI_WINDOW_EFFECTS_TRACE=1` — logs state-machine transitions to stderr.

## Phase 2 (not yet implemented)

- Metal backend (macOS): snapshot via blit from the drawable texture
  (`framebuffer_only(false)` is already set by the kept commit), composite
  pass in `metal_renderer.rs` + `shaders.metal`, `cbindgen` export for
  `TransitionParams`, `MacWindow` overrides.
- wgpu backend (Linux/Web): gated on the existing `surface_supports_copy`
  flag; `copy_texture_to_texture` snapshot + WGSL port (near-verbatim from
  `theme_transition.wgsl` in pipit-window).
- Smoke tests: macOS fullscreen + theme switch (`presents_with_transaction`
  interaction), X11/Wayland with and without `COPY_SRC`.

Until those land, `theme_transition_supported()` returns `false` on
non-Windows platforms and the synchronous theme path applies.
