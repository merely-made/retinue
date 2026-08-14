# Outrider cannot publish until Retinue does

**Date:** 2026-08-13
**Status:** RESOLVED the same day. Mark's call was to publish retinue 0.1.0
and then outrider, which is what happened: `retinue` 0.1.0 and `outrider`
0.1.0 are both on crates.io. Kept because the diagnosis is the useful part,
and because the same trap will recur the next time a workspace crate is
published from a tree that has outrun its last release.

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

## How it was resolved

Retinue 0.1.0 was published first, then outrider 0.1.0 against it. The
version was not a formality: the endpoint surface had grown enough that a
0.0.3 would have undersold it, and the release carries the workspace state
including other agents' checkpointed in-flight work.

One more stale pin surfaced on the way and is worth noting, because the
earlier survey missed it: `apps/signalman` required `retinue = "0.0.2"`.
The survey had only looked at `crates/*/Cargo.toml`, and the apps directory
is outside that glob. Cargo found it immediately; a grep across every
manifest in the repository would have found it a step sooner.

With retinue published, outrider compiled against the registry copy rather
than the path, which is the check that had been failing, and both uploads
went through.

## The cascade went deeper, and one release was defective

Publishing postilion exposed the rest of the chain, and a real defect.

**retinue 0.1.0 was broken for `tulle-radio` consumers.** Its
`iface/tulle.rs` calls `send_announcement` and
`TransmitError::AnnouncementDisabled`, neither of which exists in the
published tulle 0.0.2. `cargo publish` did not catch it, because it verifies
the package with **default features only**, and `tulle-radio` is not a
default. The default build was fine; anyone enabling that feature from
crates.io got a compile error. Postilion was simply the first thing to
exercise the path.

That is the lesson worth keeping from this whole exercise: **a green publish
proves the default feature set and nothing else.** A crate whose interesting
capability sits behind a non-default feature is published unverified in that
configuration unless someone checks it deliberately. The check that works is
to drop `path` from the dependency so cargo must resolve from the registry,
then build the feature. That was run before retinue 0.1.1 went out, and it
compiled `tulle v0.1.0` from the registry rather than the path.

The full chain turned out to be five deep, each level needing the one below
published first:

`selvage` 0.1.0, `tulle` 0.1.0, `retinue` 0.1.1, `outrider` 0.1.1,
`postilion` 0.1.0.

selvage was needed because published 0.0.1 lacked the `kiss` module and the
UI-snapshot surface that local tulle imports.

**A dev-dependency cycle blocked the last two.** Outrider dev-depends on
postilion, and postilion depends on outrider, so with a version named on
both neither could publish first. Outrider's postilion dev-dependency is now
path-only, which cargo strips from the published manifest. That is the right
shape permanently, not a workaround: a versioned dev-dependency on a crate
that depends back is a cycle waiting to recur.

## State

- `selvage` 0.1.0, `tulle` 0.1.0, `retinue` 0.1.1, `outrider` 0.1.1 and
  `postilion` 0.1.0 on crates.io. retinue 0.1.0 is superseded rather than
  yanked; it is sound on default features and only its `tulle-radio`
  configuration was broken.
- radio-face stays a dev-dependency, which was the right placement
  independent of publishing.
- Postilion still requires outrider 0.1.0 by path and is itself unreleased
  at 0.0.1; its own release is a separate decision.
