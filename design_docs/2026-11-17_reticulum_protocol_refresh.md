# Reticulum 1.6 network stamps and LXMF 1.3 costs

This refresh follows the signed RNS 1.6.0 / LXMF 1.3.0 oracle qualification.
The implementations now support network stamp emission and enforcement, and
fractional LXMF delivery and propagation costs. The owning open-work plan is
[wire compatibility](2026-11-02_wire_compatibility_plan.md).

## Source and release identity

The exact RNS and LXMF wheels came through the publisher's Reticulum service
and matched manifests signed by `bc7291552be7a58f361522990465165c`.
Their hashes remain in `crates/retinue/oracle/requirements.txt`.
The later signed release query still reported RNS 1.6.0. The LXMF 1.3.0
manifest retained from the first intake remains verified; the later latest
query did not complete. Source adaptations are listed in both crates' NOTICE
files with their source versions and locations.

The LXST 0.6.0 manifest also arrived through Reticulum. Its complete wheel
was obtained through HTTPS after bounded Reticulum transfer failures and
verified against that same publisher-signed manifest. It was inspected
without installation or execution. LXST is a separate live media protocol;
this update does not add a media implementation. Its package requires RNS
1.6 and LXMF 1.3. The stock RNode updater still names firmware 1.86.

RNS 1.6.1 and LXMF 1.3.1 (November 18) were read from their PyPI wheels,
`rns-1.6.1-0-py3-none-any.whl` (`f276cb7d…9547959`) and `lxmf-1.3.1-py3-none-any.whl`
(`42795725…ec86d9416`). PyPI's 1.6.0 and 1.3.0 wheels are byte-identical to the
signed pins, but the 1.6.1 pair has not been checked against a signed manifest,
so `requirements.txt` still pins 1.6.0.

## Retinue

`network_stamp::NetworkStampPolicy` controls destination/path emission,
link stamping, enforcement, costs, generation budgets, timeouts and cache/job
capacities. Defaults follow RNS 1.6.1: announces, path requests and requested
link requests are stamped; enforcement is off. Per-destination link costs can
be configured. Node keeps emission off until its shell provisions a stamp.
Murm's `ReticulumTransportBuilder` takes a policy (`network_stamps`) and an
RNS-style storage path (`stamp_storage`).

Stock 1.6.1's config can only switch stamping on: `stamp_destinations = No`
leaves the new default in place. The stock harness turns emission off in code.

Signed announce extensions carry the destination stamp and fractional link
cost. Signature validation covers the original extension, application data
and ratchet. A permissively accepted extension with insufficient work remains
application data and does not update the peer's link cost, as stock behaves.

Path stamps bind the target and request tag. Link stamps are embedded in the
ephemeral keyset. Enforcement runs before rejected traffic can learn a route,
consume a path tag or create a transport bridge. Existing packet, announce
rate, IFAC, route and capacity policies still apply.

Path and link stamps run in bounded blocking jobs under the configured
attempt/time budget, like stock's `pr_stamp_timeout` and `lr_stamp_timeout`.
A destination stamp is mined once on every core with no time budget, as stock
mines it; shutdown, a policy that stops stamping at that value, or the attempt
bound ends the search. Until it exists the destination does not announce, and
repeated announces coalesce. A completed job rechecks current policy and cache
capacity before emission. A required stamp which cannot be generated does not
fall back to an unstamped request or announce.

Node remains clock-free. A shell provisions its public destination stamp;
`Node::set_destination_stamp` verifies it. Link generation derives bounded
candidates from the caller's fresh per-attempt secret seed. A shell needing a
wall-clock timeout can use the cancellation-aware primitives directly.
`Node::try_announce` reports missing work and includes extension bytes in the
carrier budget. The unchecked fixture builder remains available.

`Endpoint::set_destination_stamp_store` keeps mined stamps across restarts.
`StampDirectory` writes stock's `storagepath/stamps/<hash>.dstamp` files, so a
stamp mined by stock RNS on a faster machine loads here, and a loaded stamp is
revalidated against the current material and target. Without a store, an
endpoint mines again after each restart. Node provisioning belongs to its shell.

## Outrider

`StampCost` represents an effective cost on stock's 1/256 grid. Conversion
rejects non-finite input and follows stock's cap and rounding. Validation uses
the exact integer threshold table and inclusive digest comparison rather
than a rounded display value.

Delivery announces retain their legacy rounded-up second item and append the
fractional fourth item. Propagation announces retain the legacy sixth triplet
and append the fractional eighth triplet. Decoding validates the legacy
structure even when the fractional values take precedence.

Propagation policy preserves configured amounts until subtraction of
flexibility, then quantizes the resulting floor. Separately quantizing the
operands can weaken admission by one grid step for off-grid values.
Integer stamp utilities and tickets retain their existing behavior.

LXMF 1.3.1 receivers accept a stamp over the bare material, the RNS 1.6
network stamp form, before trying the expanded workblock; stock marks this
transitional until RNS 1.6.2. Outrider's delivery and propagation admission do
the same (`stamp::bare_cost`), checked against stock-mined stamps of both
forms. Outrider still mints expanded stamps, which every stock 1.3 receiver
accepts. LXMF 1.3.1 also adds a peering-key form; Outrider has no peering.

Public delivery/admission cost fields now use `StampCost`; propagation policy
exposes configured amounts separately from the effective stamp floor.
Postilion and Signalman's local consumers were updated. Reported historical
integer work scores remain lower-bound metadata; they do not decide
fractional admission.

## Evidence

Receipts are under `Code/testing/murm/testing/receipts/`:

- `rns-network-stamps`: source digests and independent stock wire fixtures.
- `lxmf-fractional-costs`: fractional delivery, opportunistic, propagation
  and ticket tests and source provenance.
- `reticulum-refresh-stock`: signed intake, exact stock boundary comparisons,
  reciprocal stamped exchanges and positive/negative admission tests, and the
  integer/fractional delivery and propagation exchanges.
- `reticulum-refresh-integration`: preserved primary WIP, integration checks,
  aggregate Rust tests and the repeated default-settings interoperability run.

These are software and loopback observations. Physical RF, installed binaries,
cross-machine operation, full public-network traffic and LXST media are not
qualified by them.
