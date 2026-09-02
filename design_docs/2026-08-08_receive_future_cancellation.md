# Cancelling the receive future: the current split and remaining proof

**Date:** 2026-08-31 update to the 2026-08-08 finding. **Status:** the
bounded software fix is implemented in the current worktree and independently
reviewed; physical and live ownership proof remains open.

The original finding was raised by review of `bf3f820` as finding #6. It is
now stale as a description of the V4 receive loop, but its rule remains:
never race the whole `lora.rx()` future against host input.

## The current split

The V4 RNode path now follows the driver's safe phases:

```rust
prepare_for_rx();
rx_arm();
select(host_read, wait_for_irq());
rx_collect(); // only after the radio won
```

The direct V4 loop and the `rf-sleep-proof` loop use the same boundary. They
select or poll only the cancellation-safe `wait_for_irq` phase and collect the
frame afterwards without racing that collection. RNode remains an exclusive
compatibility mode, not a resident protocol lease.

`lora-phy` still exposes `LoRa::rx` as one future, but its internal phases are
important:

```rust
do_rx(listen_mode).await?;              // SPI: SetRx
loop {
    wait_for_irq().await?;              // interrupt waiter
    process_irq_event(..).await         // SPI: read and clear IRQ status
    get_rx_payload(..).await            // SPI: read the frame out of the FIFO
    get_rx_packet_status().await        // SPI
}
```

Host input can safely interrupt the idle IRQ wait. It must not interrupt the
post-IRQ SPI work, where the interrupt has been consumed and the SX1262 FIFO
payload still needs to be read. The current implementation makes that boundary
explicit;
it does not make the whole receive future safe to cancel.

## V4 DIO1 low-power registration

The low-power DIO1 waiter now uses RAII cleanup when its registration future
is dropped. It also performs the high-level handshake in the same poll as
registration. This is intentionally a narrow hardware claim: it relies on
exclusive ownership of GPIO14 and the SX1262's level-latched DIO1 behavior.
The pure registration model is covered 2/2.

## Verification and limits

The final software checks passed for the default V4 image,
`host-uart-low-power`, and `host-uart-low-power+rf-sleep-proof`, using the
Xtensa target, locked dependencies, `-Zbuild-std=core`, and `-j1` through
`rustup run esp cargo check`. Independent review accepted this bounded
software claim.

This is not a flash quiet boundary, a board radio owner, a `QuietWindow`, or
`ControlRuntime` wiring. There is no physical, light-sleep, or on-air receipt.
Those remain required before firmware can claim durable live reconfiguration.
WN1 therefore remains Partial and WN2 remains Open.

## Why the split is narrower than a permanent receive task

Keeping `lora.rx()` alive across loop iterations would still borrow `lora`
mutably while transmission needs it. A dedicated Embassy task owning the
radio remains a possible later ownership shape, especially for a resident
Retinue node. The current implementation addresses the specific V4 frame-loss
race at the receive-phase boundary without claiming that larger ownership
refactor.
