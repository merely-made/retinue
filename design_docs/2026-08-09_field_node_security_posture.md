# Field Node Security Posture

Design doc, 2026-08-09. Source: an agent security review Mark brought in,
verified against the codebase and reconciled with the standing docs. Answers
"how smart may a node get" and "what does a seized node cost." Composes with
the [channel murmuration design](2026-08-09_channel_murmuration.md) and the
[mesh scaling doc](2026-08-09_mesh_scaling_and_asymmetric_routing.md); the
authority model is the firmware-tier instance of mere's
[boundary, identity, and grant composition](../../mere/design_docs/moothold_docs/research/2026-08-09_boundary_identity_and_grant_composition.md).

## The answer to "how smart"

Smart enough to verify, too poor to authorize. Capability on the node is safe
to grow so long as no secret capable of authorizing anything grows with it.
The RNode personality's dumb profile is not the ceiling; the ceiling is what
the flash may contain.

The convergence is worth recording: this posture independently re-derives
three rules the boundary doc already holds. Transport-independent
authorization is the Stickleback rule at the firmware tier (never infer
authority from transport access). Public-keys-down with signed envelopes is
grants-as-data (authority carried as data, valid however it arrived).
Bootloader verification against a key the application cannot rewrite is the
floor-cannot-be-zero argument, including the demand that the unvotable part
be the most legible. Two design sessions reached the same laws from opposite
ends of the stack.

## The rules, in order

**1. The node's identity is worthless.** A field node holds its own relay
keypair and nothing else. Never an operator identity, never a shared fleet
identity. A seized node then buys the attacker one impersonable relay,
revocable by de-listing, and every pole stops being a key custody site. This
extends murmuration design rule 4: relay identity lives on the pole, operator
identity lives in signalman, never both on one flash.

**2. Authorization flows one direction: public keys down.** The node stores
operator public keys and verifies against them. It holds no secret that
authorizes anything. Unbolting it and dumping the flash yields a public key
that was already public.

**3. Sign commands, not sessions.** A signed envelope carrying a monotonic
counter, checked against the stored allowlist. A hijacked link yields
nothing, and commands can route through relays nobody trusts, which a mesh
requires and which murmuration visits exploit directly: a command can arrive
over a foreign bearer (port 76 over Meshtastic) and adding a channel to the
registry never adds an auth path. This is the auth complement to murmuration
rule 5's border-gateway policy.

**4. Transport-independent authorization.** Serial, LoRa, BLE, WiFi all carry
the same envelope to the same verifier. A future BLE maintenance lane then
never becomes the weakest credential in the fleet, despite being the one
physically closest to the attacker.

**5. Config authority and firmware authority are different keys.** The config
key may be warm (signalman's tier). The firmware signing key stays offline
(linkboy's tier). Verification happens in a bootloader against a key the
application cannot rewrite. On ESP32-S3 that is secure boot v2 plus flash
encryption with eFuse-burned keys. On nRF52840, APPROTECT has a real
voltage-glitch history: assume T114 flash is readable and design around that,
not around the datasheet.

**6. Non-key flash contents obey the same rule.** Site inventory, host
network credentials, contact info: none of it belongs on the pole. It lives
in signalman on the host. Standing rule: every settings-record field is
classified public / pseudonymous / forbidden at the moment it is added.

## The fleet-key liability is absent by posture, not mitigation

FCC v1 is stock hardware plus user-flash: there is no remote firmware
authority today because linkboy runs at the owner's bench
([flashing plan](2026-08-08_linkboy_public_flashing_plan.md),
[FCC posture](2026-07-20_fcc_reselling_flashed_radios.md)). A single remote
update authority over N sold radios, compromised, is a recall plus an FCC
problem. Therefore rule 5 is not one hardening item among many; it is the
precondition for any OTA lane existing. **Gate: no over-the-air firmware
update capability ships before the key split, offline custody, and bootloader
verification are in place.**

## Corrections made against the code

- **The path table is already hardened on the host tier.** TTL, capacity
  bound, dead-first-then-stalest eviction, and an eviction stat exist in
  `crates/retinue/src/endpoint.rs`. The remote-crash-primitive concern
  survives only as the firmware-tier question on 256 KB parts, still open.
- **The panic surface is real work.** 96 `unwrap`/`expect`/`panic!` sites in
  tulle's src at time of writing, unsorted. Many are the infallible
  mutex-lock convention; nobody has separated those from reachable ones.
- **Doctrine reconciliation:** diagnostics assert invariants loudly, and on
  the RF parse path a loud assert is a remote reset button. The split:
  assertions stay loud on the host tier; the on-metal RF decode path returns
  errors and counts them, never panics. Panics are jammers.

## Priced now rather than discovered later

- **Expiry needs a clock, and that is the harder half.** A field node has no
  RTC, no trustworthy time at reboot, and sits on a mesh with hour-scale
  delivery. Wall-clock expiry is therefore the wrong primitive. Either
  acceptance windows key off the counter, or the beacon grows a loose time
  term. Decide before the envelope format ships.
- **Counter persistence collides with the flash discipline.** Retinue-small
  decision 4 lands flash writes at reboot boundaries; a per-command counter
  write is a deliberate exception. The technique is standard (nRF52 flash
  allows word-writes between erases, so a wear-leveled slot log in a
  dedicated page), but the exception and its erase-cycle budget get priced in
  the design, not found in the field.

## The seizure paragraph (design target)

> A seized node yields its own relay keypair, the operator's public keys, a
> channel registry, a region table, and a replay counter. The keypair
> impersonates one relay until it is de-listed. Everything else was already
> public. Site inventory, contacts, and network credentials live in signalman
> on the host, never on flash. If the owner has granted position disclosure to
> specific identities, the node also yields how many grants exist and, for any
> identity the seizer already knows, whether it was one of them. It never
> yields the list.

Every clause is currently true or cheap. The work is keeping it true as V4
features arrive, WiFi credentials especially. When this paragraph needs a
mitigations appendix, the design has drifted.

The position-grant clause was added 2026-09-02 with PD0 below. It is an
inventory item, not a mitigation: the table is stored blinded under a
node-local secret precisely so that the sentence above stays true, and the
residual cost it names is stated rather than hidden.

## Feature targets

**FS1: Panic-free RF parse path.** Sort the existing sites (infallible-lock
convention vs reachable), convert the on-metal decode path to error returns
with counters, stand up a fuzz harness for the frame decoder.
*Validation:* fuzzer runs a sustained corpus with zero panics; a per-file
audit receipt separates convention sites from reachable ones; a malformed
frame on hardware increments a counter and nothing else.

**FS2: Signed command envelope.** Operator-signed envelope with monotonic
counter, verified against the stored allowlist; one verifier for every
transport; counter-window acceptance rather than wall-clock expiry unless the
clock question resolves otherwise.
*Validation:* a replayed command is rejected across a reboot; a command
delivered over a foreign bearer verifies identically to serial; possession of
a live session without the key authorizes nothing.

**FS3: Durable counter.** Wear-leveled slot log in a dedicated flash page,
with the reboot-boundary exception documented and the erase budget computed.
*Validation:* power cut mid-write recovers monotonicity; documented
worst-case command rate stays inside the part's erase-cycle life.

**FS4: Key custody split.** Config key warm, firmware key offline, bootloader
verification the application cannot rewrite; secure boot v2 path on V4;
T114 designed as flash-readable.
*Validation:* a modified application image fails to boot on V4; rewriting the
stored verification key from application code is demonstrated impossible; the
OTA gate above is enforced in the linkboy/signalman release checklists.

**FS5: Seizure review.** The settings-record schema carries the
public / pseudonymous / forbidden classification per field; flash contents
enumerated against the seizure paragraph.
*Validation:* a flash dump of a current image contains exactly the
paragraph's inventory; schema additions without a classification fail review.

**FS6: Firmware-tier table bounds.** The host-tier path table discipline
(TTL, capacity, eviction) restated for the 256 KB parts, with announce
flooding as the adversarial load.
*Validation:* sustained announce flood on a T114 plateaus memory and the node
keeps relaying; matches the scaling doc's FT2 validation on-metal.

**PD0: Position disclosure tiers.** Ruled here, tracked in the
[position disclosure plan](2026-09-01_position_disclosure_plan.md). Added
2026-09-02.

Three tiers, and two carriers that cannot be merged because an announce is a
broadcast and cannot vary by recipient.

*Broadcast tier.* One public value in the announce, owner-set: **off**,
**coarse**, or **precise**. Precise-to-everyone is a legitimate setting and not
a degraded one: a beacon that wants to be found, a public asset, an emergency
deployment. The owner chooses; the firmware does not editorialise. The
default on a fresh commission is **off**.

*Directed tier.* Per-identity precision, answered over a link to a named
destination, consulting a bounded owner-signed table of address hash to level.
An identity absent from the table receives the broadcast tier, never more.
That is whitelist semantics and it is the default; blacklist semantics are a
field the owner may set, not a convention the node infers. A node with no
table at all answers everything at the broadcast tier and never goes silent.

*What coarse does not promise.* Coarse is quantised at the source, before
encoding, never full precision truncated at render time. It is obscured against
a single observation. It is **not** obscured against sustained observation by
several receivers logging signal strength, which can trilaterate it back
toward precise, and that apparatus is exactly what the coverage-mapping gate
builds. Broadcast rate is therefore part of the disclosure surface, not a
separate tuning knob, and the tier is named coarse rather than safe on purpose.

*Storage.* The table lives on the node so that a headless placed node carries
and enforces its owner's decisions with no host. That puts trusted identities
on flash, which the seizure paragraph forbids for contacts. Resolution: entries
are stored blinded, as a keyed hash of the address hash under a node-local
secret, so a dump cannot enumerate who was granted. A seizer who already holds
a candidate identity can still test it, because the secret is on the same
flash; that residual is stated in the seizure paragraph rather than mitigated
away. The alternative, keeping the table host-side only, was rejected because
it removes the headless property the feature exists to deliver.

*Authority.* The table is a command to a running node and rides the FS2
envelope under config authority. Activation is monotonic-forward with no
autonomous rollback, safe only because the table never governs command
authority; see PD2 in the plan for why those two facts are inseparable.

*Validation:* the same asker receives different answers by tier and the
broadcast value is independent of any directed grant; a flash dump yields a
grant count and no identity; a coarse value is shown underivable from what is
transmitted; the absent-identity and no-table cases each return the broadcast
tier and never silence.

## Open questions

- Counter-window vs beacon-time expiry (the clock question above).
- Whether the command envelope and mere's petition wire shape unify, given
  the operator-key-down = grant, signed-command = petition mapping. The R4
  request/HMU/proof formats should at least not preclude it.
- Allowlist lifecycle: de-listing a seized relay and rotating operator keys
  are themselves signed commands, so the bootstrap and lockout stories need
  writing (who signs the command that revokes the only key).
- Whether BLE maintenance access ships at all before FS2, given rule 4.
- Position disclosure, from PD0: whether the coarse quantisation grain is fixed
  or owner-selectable, and if selectable, whether the chosen grain is itself
  disclosed. And whether a group-encrypted precise blob in the announce is
  worth its revocation cost, since removing one identity rekeys the whole set.
  Both share the allowlist-lifecycle gap above: revocation is a signed command,
  and the lockout story is not yet written.
