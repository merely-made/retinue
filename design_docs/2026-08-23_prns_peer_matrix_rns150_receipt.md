# Prns peer matrix at RNS 1.5.0

**Date:** 2026-08-23 local / 2026-08-24 UTC
**Status:** H8 Peer lane re-receipted at the current pin
**Supersedes:** the version claim of the 2026-08-11 receipt, not its lane boundary

The oracle re-pin of 2026-08-23 (`d93751b`) moved the compatibility target from
RNS 1.4.2 to 1.5.0. H8's software receipt was taken on 2026-08-12 against 1.4.2,
so it no longer described the version Retinue claims compatibility with. This
re-runs the three-corner matrix at the current pin. Nothing about Prns, the
donor ledger, or the lane boundary changed.

## Peer boundary

Unchanged from August, deliberately. The peer is a clean detached Prns worktree
at `72b6b30d27cac910ce20d370e1dc711fe9b95955`, built as `prnsd 0.3.4` and run as
a separate process. Retinue has no Prns dependency and the driver reads no Prns
library API. Stock RNS is 1.5.0 from the oracle virtualenv, whose pin is
`rns==1.5.0` / `lxmf==1.1.1`; the matrix itself drives only RNS.

| artefact | this receipt | 2026-08-12 |
| --- | --- | --- |
| peer revision | `72b6b30d` | `72b6b30d` (same) |
| daemon version | `prnsd 0.3.4` | `prnsd 0.3.4` |
| daemon SHA-256 | `8e3f25e35473b8c06e7c3b7db1ca5e22f3071fdd939d86316180b3b0e52e1cc2` | `5ef0cfbcc20b...` |
| Retinue `interop_tcp` SHA-256 | `177e717451e622bda87f11dd983376c4862ac1d1794cfb093b977cb285b2005f` | identical |
| driver SHA-256 | `76717d2d2ea61a68966c7c8076028c75c85ff6f468b8ebefc194889108e46fb2` | `7433a46fd338...` |
| Retinue revision | `d0d31c3` | `e5ac6c87` |
| stock RNS | 1.5.0 | 1.4.2 |

Two of those differences need stating plainly rather than being left to
inference.

**The August daemon binary is gone.** It was overwritten in the temporary target
directory during this work, before the build defect described below was
understood. Peer-*binary* continuity with August is therefore broken and this
receipt does not claim otherwise. What is continuous is the peer *source*: the
same commit, the same `Cargo.lock`, and the same toolchain, which has been
stable 1.97.1 since 2026-07-22.

**The Retinue side is byte-identical.** The `interop_tcp` example executed here
has the same SHA-256 as the one August ran, despite the repository revision
moving. That is the strongest continuity anchor available: the Retinue half of
the comparison is provably the same bytes on both sides of the version change.

The driver digest moved because four dead `repos/Prns` paths were repaired in
`d0d31c3`; the peer has lived at `crates/prns` since the workspace split forks
out of `repos/`.

## Matrix result

Seven runs at RNS 1.5.0, all pass, 35 of 35 case executions. Three were taken
against a clean tree at `d0d31c3` and are the receipt runs:

| run | `matrix.json` SHA-256 |
| --- | --- |
| `peer-20260824T022247Z` | `318b5ab936690d47e233515d9884659b8328fd45d53530eb1cb2ffdba26a62bd` |
| `peer-20260824T022318Z` | `b3d3474c737db761fe8e894bf05429ed062465c03d3e629d065face7966b0138` |
| `peer-20260824T022340Z` | `d450c8271dc201d91ef009234f37719878dbf17571799dbfafaa2056c0e0612f` |

Four earlier runs (`021646Z`, `021857Z`, `021914Z`, `021928Z`) used byte-identical
driver and daemon but ran before the driver fix was committed, so their recorded
Retinue revision does not contain the driver they executed. They are reported
here because they are part of the sample, not as independent receipts.

Result directories are intentionally gitignored: they hold transient identities,
ports, and raw captures.

| Case | Result | Receipt |
| --- | --- | --- |
| Retinue and stock RNS | Pass 7/7 | Announce validated in both directions on every run. |
| Retinue and pinned Prns | Pass 7/7 | Prns learned Retinue's destination through its path table; Retinue validated Prns's announce. |
| pinned Prns and stock RNS | Pass 7/7 | Stock RNS validated Prns's `nomadnetwork.node` announcement; Prns learned the stock destination. |
| stock RNS transport O-10 | Pass 7/7 | Two leaves observed one another as type-2 transport announces at `hops=1`. |
| Prns transport O-10 | Pass 7/7 | The same capture shape, `hops=1`, `header=Type2`. |

## What the captured byte counts do and do not show

The August receipt quoted capture sizes as though they were characteristic of
the pairing, as "188 bytes Retinue to RNS and 386 bytes RNS to Retinue". **They
are not constants.** They are single samples of a quantity that varies run to
run, and reading a version delta out of them is a mistake this receipt nearly
repeated.

The first 1.5.0 run measured 189 and 382 against August's 188 and 386, which
looks exactly like a wire change in the new RNS. Three more runs produced 189,
188, 188 and 384, 381, 381. Across all seven runs every stream varies and every
capture has a distinct digest.

The cause is HDLC framing, not the protocol. `crates/retinue/src/iface/hdlc.rs`
byte-stuffs `0x7E` to `7D 5E` and `0x7D` to `7D 5D`. Each run mints fresh
ephemeral identities, so each run's destination hashes, keys, and signatures
contain a different number of bytes that need escaping. The relationship holds
exactly in all measurements: raw length equals unstuffed length plus escape
count plus frame delimiters.

Unstuffing the captures recovers the invariant, and it is the same on both sides
of the version change:

| stream | 1.4.2 raw | 1.5.0 raw range | unstuffed |
| --- | --- | --- | --- |
| Retinue to stock RNS | 188 | 188-190 | **185** |
| Retinue to Prns | 188 | 188-190 | **185** |
| stock RNS to Retinue | 386 | 381-385 | **376** |
| stock RNS to Prns | 382 | 381-386 | **376** |
| Prns to Retinue | 202 | 202-203 | **199** |
| Prns to stock RNS | 202 | 201-204 | **199** |

Every unstuffed length is constant across all eight runs, 1.4.2 and 1.5.0 alike.
That is a stronger compatibility statement than the August receipt made, and it
corroborates the re-pin's finding that the wire has not moved across
1.3.8 to 1.4.2 to 1.5.0, now from the peer matrix rather than from fixtures.

Anyone re-reading a raw byte count as evidence should unstuff first.

## O-10 disposition

Unchanged. A source announce at wire hop 0 appears after one transport forward
as a type-2 packet at wire hop 1, in stock RNS and in Prns alike, on every run.
There is no local TCP discrepancy. This receipt does not promote that to a
physical result; the radio lanes still owe an RF forwarding receipt.

## Procedure defects found and fixed

The documented procedure could not produce this receipt, and the failure was
silent. Both defects are repaired in `d0d31c3`.

The peer paths were dead: `PRNS_ROOT` resolved to `repos/Prns`, which exists
nowhere, and the same location was written into the driver docstring, the oracle
README, and a hardcoded absolute path in the "no prnsd executable found" hint.

More seriously, Prns pins `link-arg=/STACK:268435456` for windows-msvc in its
own `.cargo/config.toml`, and Cargo resolves that file relative to the working
directory rather than to `--manifest-path`. The README built the peer from the
oracle directory, so the setting was dropped, the linker took the Windows 1 MiB
default, and `prnsd`, whose non-tray `main` is `#[tokio::main]`, overflowed that
stack during startup on every invocation, `--version` included. The build
reported success. Patching only the PE `SizeOfStackReserve` field of the failing
binary from 1 MiB to 8 MiB made the same bytes run clean, which isolates the
cause to stack size alone.

It follows that the August receipt was not produced by the command its README
documented, because that command cannot produce a daemon that answers
`--version`.

## What this receipt does not cover

**It is not exposed to the live-gate flake, and it is not evidence about it.**
`peer_matrix.py` drives announce exchanges and transport forwarding only. It
never runs `interop_reqresp`, `interop_resource_recv`, or `interop_ifac`, which
are the gates where flake is measured (see the 2026-08-23 re-pin receipt and
`2e73365`). The "a passing suite run is weak evidence" caveat does not transfer
to these five cases. Equally, seven clean peer-matrix runs say nothing about the
reqresp flake.

It does not close Air, Assurance, Distribution, RF range or loss, or installer
custody gates, and it is not an on-air result.

## Corrections to the August record

- The 2026-08-12 history was not two complete matrices. It was two failures
  (`034902Z`, `034948Z`, one case each), a four-case pass under an earlier
  driver (`035333Z`, driver `580b54da`), and the five-case pass the receipt
  cites (`035508Z`, driver `7433a46f`). The drivers differ; these are iterations
  of a driver being fixed, not independent confirmations.
- Capture byte counts in that receipt are samples, not constants. See above.

## Open

- **A stronger three-party transit link receipt**, still deferred: it needs a
  relay-off control and physical isolation.
- **An RF forwarding receipt** for O-10, owned by the radio lanes.
- **Whether the peer daemon builds reproducibly.** The rebuild from identical
  source, lockfile, and toolchain produced a different digest from August's. It
  was never checked, and nothing here depends on it, but a peer whose binary is
  reproducible would make future re-receipts a byte comparison.
