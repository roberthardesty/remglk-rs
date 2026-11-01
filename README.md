RemGlk-rs
=========

A Rust port of [RemGlk](https://github.com/erkyrath/remglk).

Except it's actually largely based on [AsyncGlk's GlkApi](https://github.com/curiousdannii/asyncglk), because Rust is more similar to TypeScript than C, and I like how I'd designed things there.

RemGlk-rs is MIT licenced.

AutosaveLib fork
----------------

This standalone source fork is pinned to upstream RemGlk-rs commit `34e0ed91c1d6949862409c19d1b11816debb9ade`. Its AutosaveLib consumer contract follows Glulxe 0.6.1 commit `56ab8743bab565de307bd892c555d8d8897ed517`, with C RemGlk `rgauto.c` commit `7d53a46a9d40d2e569b154e4740e8ac4eee67cb8` as the semantic reference.

`remglk::glkapi::autosave` implements a bounded, versioned, Rust-native Serde library-state snapshot. It is not compatible with C RemGlk's JSON and does not serialize Rust or C pointers. VM retained-buffer keys are addresses obtained from the interpreter's autorestore registry. The C ABI in `remglk_capi` exposes the GlkUnix save/load/update/free, serialization context, last-event, updatetag, dispatch-rock, and retained-array registry functions used by Glulxe and reusable by Bocfel.

Restores parse and validate an inactive graph before live teardown. File streams reopen through the configured `GlkSystem` (Dialog under Emglken), resource streams reopen from the current Blorb, and the resource map itself must be rebuilt by the consumer from its saved gamefile tag, as Glulxe's `recover_extra_state` does. Restored windows emit one complete structured refresh while the last-event sentinel suppresses the immediate duplicate autosave. Sound-channel playback is outside this schema.