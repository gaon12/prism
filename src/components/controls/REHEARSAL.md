# Rehearsal: live search modes

Reference: StageShow v1.1.1 `kit/controls/Segmented` and its craft/motion rules.
The shared inset track, equal columns and retained sliding thumb come from that
component. Prism tokens supply the surfaces, radii and text; actual buttons,
pressed state and keyboard behavior connect it to the real search session.

The user's brief applies StageShow's look to the working application. Its
illustration-only placeholder bars, authored cursor, staged state changes and
cropped marketing canvas are outside this control's scope.

Flow: preserve a query → change modes by pointer or keyboard → render the
corresponding search → move from a utility expression back to file search.
Arrow keys and Home/End retain focus; Tab leaves the group. The automatic-mode
explanation remains a tooltip and accessible description.

Captured: production-browser light/dark at 1280 and 390 with large English text;
native WebView2 light/dark at 1280 and 390 with actual indexed PDF results.
Screenshots are saved outside the repository under the QA environment's
`prism-ui-evidence` temporary directory. File prefixes are `stageshow-modes-`
and `stageshow-native-`.

Checks: labels remain inside the track; the settled thumb aligns with the chosen
button; reduced motion disables transitions; names, description, pressed state,
query preservation and keyboard routing pass. Native PDF results remain available
without starting the installed embedding model. Application console errors: zero.
The bundled browser suite passes 23 tests and the client suite passes 67.

Review: inspected the light/dark desktop/mobile frames and native result screens.
The control is one quiet track with a clear chosen state; long labels wrap at
large text sizes. Descriptions sit outside the buttons so their accessible names
remain concise. Focus outlines represent actual keyboard focus.

Limitations: no authored Stage or playback loop is shipped, so the illustration
`film.mjs --gate` protocol was not run. Intermediate motion frames and native
macOS/Linux rendering were not recorded. A calculator cold-start timeout occurred
during a concurrent Rust build; the unchanged test passes alone and in the full
suite after that build finishes. Heavy-load startup performance remains a separate
measurement rather than being resolved by a longer timeout.
