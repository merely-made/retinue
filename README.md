# Retinue: managed cadre of mesh protocols

Embedded Rust implementations of three mesh protocols, a shared
radio interface layer, operator apps, and firmware that runs on my limited 
subset of hardware. 

So: what's the idea? Why all three? I wanna see if it's possible, using embassy,
to make the three more like channels that are triggered by a given announce, allowing
one node to configure a persona/address for each protocol and route traffic correctly. 
My mental picture: a retinue of protocols, working together. 

On the host side, this is possible with existing projects, but on the embedded side, 
doing it would mean forking C++, which I haven't had since college. So, Rust: the path 
of many such glitzy attempts, of late. Mine is comparatively drab: I prompt, I read, I type, 
I learn something new about Rust or embedded programming, I say "don't use this" when I don't 
know what it'll do, and *don't use this,* I don't know what it'll do.

I am pathologically honest: I use LLMs because I have come to truly enjoy leveraging my 
English degree in increasingly esoteric and elaborate ways, and I won't ever release or 
promote this implementation without demonstrated wire compatibility with reference 
implementations, at the very least. That's my goal, and I'm sticking to it.

Each crate has its own README with more detail, and here's a generated overview.

## Status (2026-09-27)

- The shared Rust protocol crates serve both host applications and firmware.
  Retinue's recorded RNS 1.5.4 qualification covers links, requests, streams,
  Resources and routing within the measured local-TCP scopes. Outrider's seven
  LXMF 1.1.1 gates also passed. The [repin receipt](testing/receipts/rns-1.5.4-repin/README.md)
  records measured scope; the [wire plan](design_docs/2026-09-27_wire_compatibility_plan.md)
  records boundary repairs and remaining gates. These are not full upstream parity.
- T114 bounded transport has an on-air receipt from August 13. September's V4
  resident-protocol receipt covers retained state and ordinary protocol traffic;
  full fault, memory and unattended-operation acceptance remain open.
- Signalman desktop collected observations from identified V4 and T114 boards
  with automatic saving disabled, and keyboard export/reload was verified on
  September 21. Device-switch association clearing is implemented. Clipboard,
  scrolled accessibility, physical cancellation and synchronized coverage remain open.
- MC5 shared-radio power measurement is pending. The owner has a PPK2, currently
  disconnected; wiring, supply isolation and measurement qualification remain open.

The [canonical index](design_docs/DOC_README.md) links exact receipts and open
criteria. Historical plan queues and older README summaries are not current
execution orders. The next comparison work is to classify behavioral differences
against existing implementations and connect them to reproducible tests; it does
not imply a new source adaptation or licensing change.

## Use

```sh
cargo test -p retinue      # or any host crate by name

# Flashing tool
cargo run -p linkboy -- list
cargo run -p linkboy -- flash <device> <package>

# Radio host app
cargo run -p signalman -- <port> [name] [bw_khz] [phy|rnode]

# Firmware (workspace member, not a default member)
cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf

# Desktop GUI (own workspace root)
cargo build --manifest-path apps/signalman-desktop/Cargo.toml
```

## License

Mozilla Public License 2.0 ([LICENSE](LICENSE)), including the firmware 
(though the firmware images themselves when created are GPLv3).
Vendored third-party forks under `vendor/` (lora-phy, embedded-graphics,
embedded-graphics-core) keep their own MIT/Apache-2.0 terms.
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) aggregates derivations;
per-crate NOTICE and PROVENANCE files record specifics.

We acknowledge Mark Qvist and the Reticulum contributors. The full
[Reticulum License](RETICULUM_LICENSE) is included for reference; its scope and
our source-review policy are explained in the [notices](THIRD_PARTY_NOTICES.md#reticulum).
We respect those terms in our own use of the reference implementation. This is
an acknowledgment, not an added restriction on users of independently authored
Retinue code; its MPL-2.0 license remains unchanged.

## History

`tulle`, `sennet`, and `tucket` merged into this workspace on 2026-07-23 with
history preserved; their standalone repositories are archived.

I've been using reference implementations as oracles, capturing packets sent via my 
radios to build these implementations; the references were reviewed once but not 
copied and not translated. Why: I don't mind adhering to the terms of a license 
provided it doesn't make me change the rest of the licensing as a combined work (GPL >_>), 
or it has an established legal precedence I can point to if people ask me about it.

I have included the Reticulum license in the repo, for example, and agree to its terms 
in principle: no violence, no training/developing AI. Ok, bet, Anthropic is out of luck. 
But, as far as me imposing those violence and LLM training restrictions myself or 
instructing people who clone this repo to do so, I feel like I have reasonable concerns 
that haven't been addressed. How will other people interpret those terms? I note that 
use restrictions are not compatible with MPL-2.0, my personal standard, and I am aware 
of no legal precedence for these particular restrictions, which makes me uncomfortable. 

So I will try to take the slightly harder path for a hobbyist using LLMs, and 
will avoid copying and/or translating the reference source. This is all ancillary 
to giving my local community a reliable, resilient network, and I'm happy to use
the reference or microReticulum or whatever meets those needs. This implementation
achieving any fraction of the reference standard's utility would be cream on top.

I am willing to forfeit any of these efforts to the owners
of the reference implementations, under whatever terms they wish,
or to discuss what I should change. I acknowledge the talented folks
who made these protocols probably have a better understanding of the 
legal situation, but I gotta work with my own limited understanding.
---

*This README was generated by AI and has/will be edited by the author upon
release.*
