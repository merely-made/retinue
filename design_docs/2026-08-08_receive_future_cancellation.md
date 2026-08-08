# Cancelling the receive future: what it costs, and why the fix waits

**Date:** 2026-08-08. **Status:** characterised, deliberately not fixed yet.
Raised by review of `bf3f820` as finding #6.

## The shape

Both firmware loops race the radio against the host:

```rust
let radio_receive = lora.rx(&radio.rx, &mut radio_frame);
let waiting = select(host_read, radio_receive);
```

`select` drops the loser. When host bytes arrive first, the receive future is
cancelled wherever it happened to be.

## Where cancellation is harmless, and where it is not

`LoRa::rx` (vendored `lora-phy/src/lib.rs:264`) is:

```rust
do_rx(listen_mode).await?;              // SPI: SetRx
loop {
    wait_for_irq().await?;              // interrupt waiter
    process_irq_event(..).await         // SPI: read and clear IRQ status
    get_rx_payload(..).await            // SPI: read the frame out of the FIFO
    get_rx_packet_status().await        // SPI
}
```

Almost all wall-clock time is spent in `wait_for_irq`, and dropping there is
harmless: no transaction is open, and the radio stays in continuous receive.
That is why this has not obviously bitten us.

Two windows are not harmless:

1. **Between the IRQ firing and `get_rx_payload` returning.** Cancel here and
   the frame is *gone*: the interrupt has been consumed, the payload is still
   in the SX126x FIFO, and the next packet overwrites it. Nothing reports a
   loss, because from the firmware's point of view no frame ever arrived.
2. **Mid-SPI, with DMA in flight.** Dropping the future drops the transfer
   future. Whether that is recoverable depends on the HAL; on the nRF52840's
   SPIM and the ESP32-S3's SPI it is not a state we deliberately enter.

Per event the odds are small: the vulnerable span is microseconds of SPI inside
a wait that is usually idle. Over an unattended pilot node running for weeks,
small and certain are the same thing, and the failure is silent.

## Why the obvious fix does not work

Keep the future alive across loop iterations and only drop it when it
completes. This cannot be done here: `lora.rx()` borrows `lora` mutably, and
the same `lora` is needed to transmit. A persistent receive future would make
transmission uncompilable, which is presumably why the loop is shaped this way.

## The fix that does work

Give the radio its own Embassy task owning `lora`, with channels for frames out
and frames in. Cancellation then cannot happen, because nothing races the
receive; the host loop selects on a channel receive, which *is* cancel-safe.
This is the ordinary embassy shape for a peripheral with two clients.

It is a main-loop restructure on both boards.

## Why it is not being done today

The 2026-08-11 FIVCO demo runs on this firmware. Restructuring the loop that
carries it, with only the V4 available for hardware verification (the T114 is
currently a stock RNode paired to a phone for the demo), trades a rare silent
frame loss for a fresh and untested class of risk in the part that must not
fail. The correct order is: demo, then restructure, then a soak long enough for
the counters to mean something.

## What would make it measurable in the meantime

The loss is currently invisible. A cheap instrument: count select outcomes
where the host won *while* the radio future had already consumed an IRQ. That
needs the driver to expose "I am past `wait_for_irq`", which vendored `lora-phy`
does not, so it is not free either.

Simpler proxy, available now: on a bench with a known transmit rate, compare
`rx_ok` on the receiving board against frames sent. Any deficit that grows with
host chatter is this. Worth one run before the restructure, so the fix has a
number to beat.
