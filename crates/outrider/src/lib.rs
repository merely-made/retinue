//! Outrider: LXMF as a boundary crate in the retinue family.
//!
//! An outrider rides ahead of or beside the party to scout the road and carry
//! word. This crate carries mail for the household: an implementation of LXMF,
//! the message format and delivery system of the
//! [Reticulum](https://reticulum.network/) ecosystem, riding on
//! [retinue](https://github.com/mark-ik/retinue)'s destinations, links, and
//! resources. Not affiliated with the Reticulum or LXMF projects.
//!
//! Outrider is a boundary crate: a codec, delivery state machines, and a
//! propagation client/server, consumed by an application at its edge. It does
//! not define conversation, contact, or storage semantics; those belong to
//! the consumer.
//!
//! # Status
//!
//! Founded 2026-07-25; no wire code yet. The founding scope, provenance
//! discipline, and ordered gates live in
//! `design_docs/2026-07-25_outrider_lxmf_founding.md` in the repository. The
//! first gate is a black-box capture oracle against a pinned stock client;
//! per the household's capture-before-coding rule, no wire format is
//! implemented ahead of it.
//!
//! # Provenance
//!
//! Outrider is implemented from the public LXMF specification prose and
//! black-box observation of pinned stock clients. The Python LXMF
//! implementation and its client applications are never read. See
//! `PROVENANCE.md`.
