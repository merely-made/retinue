# Assurance lane: what passes, and one surface that has never run

**Date:** 2026-08-10. **Lane:** Assurance (ASSURE1 through ASSURE5).
**Status:** ASSURE1 and ASSURE2 verified. ASSURE3, ASSURE4, and ASSURE5
implemented; see [the FS2 carrier decision](2026-08-10_fs2_command_carrier_decision.md)
and [FS4 custody and FS5 seizure](2026-08-10_fs4_custody_and_fs5_seizure.md).
The shared source lock is cleared by the
[Prns donor ledger](2026-08-10_prns_donor_ledger.md).

## ASSURE1, validation minimum: substantially closed

`python validation/run.py verify` passes for this ASSURE1 tree: 19 owned Cargo
manifests, 66 validation assets, 12 suites. Orphan detection works, and
exact-SHA recording is *enforced* rather than merely documented: `record`
refuses a dirty worktree, and a self-test asserts that producing a result
cannot dirty its own source.

One gap found and closed. The seed glob named a single target's directory
(`fuzz/seeds/retinue-node-ingest/*`), so a second target's corpus entered the
tree unregistered. It is now `fuzz/seeds/*/*`, which caught the very next
addition, as intended. Suite asset globs stay per-target, so a suite still
cannot claim another's evidence.

## ASSURE2, ingest and unsafe boundaries: half done

**Unsafe policy: passing.** `validation/security/unsafe_audit.py` reports 14
approved tokens across 4 files, 17 first-party crate roots checked.

**Fuzzing: extended, and then found to be unrunnable here.**

A second target was added, `outrider-lxmf-decode`, because ASSURE2 asks for the
whole ingest path and only the node route was covered. This is a different
trust boundary from `retinue-node-ingest`: `outrider::portable::decode` is the
LXMF parser firmware links, it runs on a stack measured in kilobytes, and its
bounds on nesting depth and container counts are days old and have never been
attacked by anything but hand-written cases. The target drives four branches:
raw bytes, the stock 0.9.6 capture unmodified, that capture mutated, and a
payload this codec built with a fuzzer-chosen `fields` map, then damaged. The
fields map is the right thing to hand over, because it is carried verbatim and
never interpreted, so it is the byte range a hostile sender controls most
directly. It builds, dry-runs with isolated seeds, and is registered.

It has not produced a single execution, and neither has the target that
preceded it.

## The finding: the fuzz suite has never run on this machine

`cargo fuzz run` fails on Windows-MSVC for **both** targets:

- With the default sanitizer: `STATUS_DLL_NOT_FOUND` (`0xc0000135`). The ASan
  runtime DLL is absent; no `*asan*dynamic*` exists anywhere in the nightly
  toolchain.
- With `--sanitizer none`: link failure, `unresolved external symbol
  __start___sancov_cntrs` and friends. libFuzzer needs the coverage symbols the
  sanitizer runtime provides, so removing it removes the fuzzer.

There is no `validation/results/` directory, which is consistent: no fuzz
evidence has ever been recorded, at any commit.

This matters more than the missing target did. The registry lists
`retinue-node-ingest-fuzz` at `scheduled` tier, and the inventory count makes
the surface look covered. What the inventory records is that a *command
exists*, which is exactly the distinction the registry's own README draws when
it says the manifest "names commands and their owner" and does not duplicate
assertions. Read correctly it never claimed the fuzzer had run. Read quickly it
looks like coverage.

### What would fix it

One of, in preference order:

1. **Run fuzzing in CI on Linux.** cargo-fuzz is a first-class citizen there,
   the sanitizer ships with the toolchain, and `scheduled` tier suits a job
   that is not on the PR path. This also makes the evidence reproducible by
   someone who does not own this laptop.
2. **Install LLVM/clang on Windows** and put `clang_rt.asan_dynamic-x86_64.dll`
   on `PATH`. Restores local runs, but the evidence stays machine-specific.

**Option 1 is implemented.** `.github/workflows/ci.yml` gains a `fuzz` job on
`ubuntu-latest`: nightly toolchain, `cargo install cargo-fuzz --locked`, a
dry-run proving corpus isolation for both targets, then a 120-second campaign
each, with `fuzz/artifacts/` uploaded on failure so a crash arrives with its
reproducer attached. Short budgets on purpose: this is a smoke gate proving the
targets build, launch, and survive their own seeds. Real corpus growth belongs
to the scheduled tier.

A `validation-registry` job joins it, running `validation/run.py verify` and
the unsafe audit on every push, so an unregistered asset or a new unsafe token
fails the build rather than waiting for someone to run the checker by hand.

Until that job has run green once, the honest statement remains that Retinue's
ingest paths are bounded by review and hand-written tests and have not been
fuzzed. The difference is that there is now somewhere for the evidence to come
from.

## Changed here

- `fuzz/fuzz_targets/outrider_lxmf_decode.rs`, plus its seed.
- `validation/run_fuzz.py`: `--target`, with seeds resolved from the target
  name so a target cannot borrow another's corpus and report coverage it never
  had.
- `validation/manifest.toml`: widened seed glob, new suite.

A third target joined later the same day: `retinue-command-accept`, the FS2
authorization boundary. It asserts three properties rather than only absence of
panics, because two of them are the gate's actual claims: no command attributed
to an unallowlisted key is ever accepted, and no command is ever accepted twice.

## The rest of the lane

- **ASSURE3, carrier evidence: done.** Six RNS 1.4.2 signed artifacts captured
  from `rnid` and reproduced byte for byte. Details in
  [the FS2 carrier decision](2026-08-10_fs2_command_carrier_decision.md).
- **ASSURE4, command decision: done, and FS2 with it.** The compact Retinue
  envelope is normative; the signed artifact stays on the host tier. FS3 now has
  a settled grammar to bind, which is why it was sequenced second.
- **ASSURE5, custody and seizure: done in software.**
  [FS4 custody and FS5 seizure](2026-08-10_fs4_custody_and_fs5_seizure.md)
  supplies the process policy and the release checklist, and FS5's inventory is
  now an enforced check (`flash-classification`) rather than a paragraph. FS4's
  physical receipts remain Distribution's.
- **Shared source lock: cleared.** The
  [Prns donor ledger](2026-08-10_prns_donor_ledger.md) itemizes every seam with
  measured overlap figures, elects MIT inbound, and records the disclosure state
  as owed and unpaid.

What the lane has not produced is on-metal evidence. Every claim here is
host-side or CI-side. No board has verified a command over RF, and the CI fuzz
job has not yet run green once.
