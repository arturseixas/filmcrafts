# third_party

`winit/`: the Craft suite's fork of winit 0.30.13 (Apache-2.0, see its `LICENSE` and `PATCHES.md`),
vendored because the Android backend needs a change that is not in a release (a mouse's hover,
buttons and wheel, and the keyboard modifiers: `PATCHES.md` F1). The workspace `[patch.crates-io]`
points at it. It is not a workspace member and is excluded from the lints.
