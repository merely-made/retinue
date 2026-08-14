# Outrider cannot publish until Retinue does

**Date:** 2026-08-13
**Status:** Blocker record. Outrider is bumped to 0.1.0 in-tree and is
otherwise ready; `cargo publish` is blocked on a dependency, not on
Outrider's own code.

## What was attempted

Publishing `outrider` 0.1.0, carrying the voice module, the Pipit clip
carriage, and the LXMF field capture. The crate is at 0.0.1 on crates.io.

## Three findings, in the order cargo surfaced them

**1. Postilion pinned the old requirement.** `postilion` required
`outrider = "^0.0.1"`, which the bump broke inside the workspace. Updated to
`0.1.0`. Postilion's own published 0.0.1 still references the old outrider
and is unaffected.

**2. radio-face was an optional dependency and is unpublished.** Cargo
requires a version, and registry presence, for optional dependencies too.
But the library never used it: `radio_face` appears nowhere in
`crates/outrider/src`, only in the `direct_phy_ui` example. It is now a
dev-dependency, where a path-only entry is stripped at publish time. That
removed the blocker and is the correct placement regardless: a boundary
crate should not carry an example's dependency in its public graph. The
`tulle-radio` feature no longer names it. The radio examples still build.
The same is true of `tulle`, which the library also never uses, but `tulle`
is published so it blocks nothing; left alone to keep this change small.

**3. The real blocker: Outrider needs unpublished Retinue APIs.** When
packaging, cargo resolves a path-plus-version dependency from the registry,
not the path. Against the published `retinue` 0.0.2 the crate fails to
compile with 22 errors, among them `no method named send_single found for
&Endpoint`. That method exists locally at `crates/retinue/src/endpoint.rs`
and appears zero times in the published 0.0.2, so this is a genuine API gap
rather than a manifest mistake. Outrider has been developed against a
Retinue that has moved a long way past its last release.

## What this means

Publishing Outrider requires publishing a Retinue that carries the APIs it
uses. That is a larger decision than it sounds and belongs to Mark:

- Retinue is the workspace's flagship crate, and its working tree currently
  holds a large multi-lane checkpoint including other agents' in-flight AIR,
  Linkboy and Signalman work. Publishing it now publishes that state.
- The version has to be chosen deliberately. The endpoint API grew, so a
  0.0.3 that quietly changes the surface would undersell it.
- Everything downstream inherits the choice: Outrider's manifest needs the
  matching version, and Postilion's release story follows behind that.

Until then, `retinue` stays a path-only dependency in Outrider's manifest.
Adding `version = "0.0.2"` there would assert a compatibility that does not
exist, which is worse than being unpublishable.

## State left behind

- Outrider is `0.1.0` in-tree, Postilion requires it as such, and the
  workspace builds and tests clean.
- radio-face is a dev-dependency, which is the fix worth keeping whatever is
  decided about publishing.
- No crate was published. The publish is one `cargo publish` away once a
  suitable Retinue release exists.
