//! Signalman: the radio-management application of the retinue family.
//!
//! The signalman sits in the box, sets the routes, works the block sections, and hands out
//! the single-line token — the object exactly one train may hold to enter a single-track
//! section. That is this hardware's channel model by another name: one radio, one mesh at a
//! time, admission by an explicit act. This application is the signal box for a household's
//! radios: which board runs which channel, who is heard, what is delivered, and what the
//! boards themselves report.
//!
//! # Status
//!
//! Founding stub. The name is claimed and the role is decided; the application arrives by
//! absorbing the `park` example on top of the [`postilion`](https://crates.io/crates/postilion)
//! host library. See the retinue repository's design docs.
