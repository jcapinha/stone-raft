# stone-raft

Personal experiment: a synthesizer that runs on an Android phone as a standalone instrument you sideload yourself. Built in Rust as a learn-as-you-go first Rust project. Never published to the Play Store or any other app store.

The author knows Python and data pipelines well, and does not yet know Rust or similar systems languages. There is no traditional software-engineering background. Agents should explain trade-offs in plain language and teach while deciding.

## Language

**Host**:
The environment that loads and runs the instrument (for example a DAW on desktop, or this app’s own shell on Android).
_Avoid_: runner, container

**Voice**:
One sounding note or layer the synth is generating at a moment in time.
_Avoid_: channel (unless meaning audio output channel)

## Decisions

**Rust as the implementation language**
The project is intentionally a first Rust codebase. The goal is to learn the language by building something real (audio on a phone), not to ship the fastest prototype in a familiar stack. Python stays a useful analogy for agents when explaining concepts, not a candidate runtime for the synth engine.

**Android phone as the primary target**
The instrument should be usable on the author’s Android phone as a standalone app. Desktop or plugin hosts may appear later as development aids, but the product intent is “play it on the phone,” not a desktop-first plugin that happens to also build for mobile.

**Personal sideload only — never app stores**
Distribution is limited to installing the build on the author’s own devices (sideload). There is no plan to publish on Google Play or any other store, now or later. That removes store compliance, signing-for-release-to-public, and store UX constraints from the design space.
