//! Read-only, bounded projection of owner RAM into direct-PHY responses.
//! This module cannot borrow a radio, clock, durable store or control runtime.
use super::{
    owner::OwnerObservations,
    recorder::{CursorError, CursorRequest},
};
use selvage::observation::{CursorReply, ProfileReply, Reply, Request, Status};

const _: () = assert!(super::MAX_RECORD_BYTES == selvage::observation::MAX_OBSERVATION_RECORD_LEN);

pub fn invalid_reply() -> Reply {
    Reply::Cursor(empty_cursor(0, Status::InvalidRequest))
}

fn empty_cursor(request_id: u32, status: Status) -> CursorReply {
    CursorReply {
        request_id,
        status,
        boot_id: 0,
        oldest: 0,
        newest: 0,
        next: 0,
        recorded: 0,
        overwritten: 0,
        encode_failed: 0,
        profile_count: 0,
        record_len: 0,
        record: [0; super::MAX_RECORD_BYTES],
    }
}

/// Discovery uses boot zero and cursor zero. Subsequent reads require the
/// discovered boot identity. Each request returns at most one record or gap.
pub fn reply(owner: Option<&OwnerObservations>, request: Request) -> Reply {
    match request {
        Request::Cursor {
            request_id,
            boot_id,
            after_sequence,
        } => {
            let mut result = empty_cursor(request_id, Status::Disabled);
            let Some(owner) = owner else {
                return Reply::Cursor(result);
            };
            let recorder = owner.recorder();
            let bounds = recorder.bounds();
            let stats = recorder.stats();
            result.boot_id = bounds.boot_id;
            result.oldest = bounds.oldest_available.unwrap_or(0);
            result.newest = bounds.newest_available.unwrap_or(0);
            result.recorded = stats.recorded;
            result.overwritten = stats.overwritten;
            result.encode_failed = stats.encode_failed;
            result.profile_count = owner.profile_count();
            if owner.disabled() {
                return Reply::Cursor(result);
            }
            result.status = Status::Ok;
            if boot_id == 0 {
                if after_sequence != 0 {
                    result.status = Status::InvalidRequest;
                }
                return Reply::Cursor(result);
            }
            match recorder.drain(CursorRequest {
                boot_id,
                after_sequence,
                max_records: 1,
            }) {
                Ok(mut drain) => {
                    result.next = after_sequence;
                    if let Some(record) = drain.next() {
                        match record.encode(&mut result.record) {
                            Ok(length) => {
                                result.record_len = length as u8;
                                result.next = drain.next_cursor();
                            }
                            Err(_) => result.status = Status::Disabled,
                        }
                    }
                }
                Err(CursorError::WrongBoot { .. }) => result.status = Status::WrongBoot,
                Err(CursorError::FutureSequence { .. }) => result.status = Status::FutureCursor,
            }
            Reply::Cursor(result)
        }
        Request::Profile {
            request_id,
            boot_id,
            profile_id,
        } => {
            // Keep the immutable dictionary available even if recording later
            // disables: previously retained events still refer to these ids.
            let mut result = ProfileReply {
                request_id,
                status: Status::Disabled,
                boot_id: 0,
                profile_id,
                config: [0; 16],
            };
            if let Some(owner) = owner {
                result.boot_id = owner.recorder().boot_id();
                result.status = if boot_id != result.boot_id {
                    Status::WrongBoot
                } else if let Some(profile) = owner.profile(profile_id) {
                    match selvage::encode_config_command(profile) {
                        Ok(config) => {
                            result.config = config;
                            Status::Ok
                        }
                        Err(_) => Status::Disabled,
                    }
                } else {
                    Status::UnknownProfile
                };
            }
            Reply::Profile(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::ObservationRecord;
    #[test]
    fn discovery_and_pages_are_read_only_and_exact() {
        let mut owner = OwnerObservations::new(9).unwrap();
        for time in 0..40 {
            owner.rx_damaged(time, selvage::PhyProfile::meshtastic_long_fast(915_000_000));
        }
        let before = owner.recorder().stats();
        let request = Request::Cursor {
            request_id: 1,
            boot_id: 9,
            after_sequence: 0,
        };
        let first = reply(Some(&owner), request);
        assert_eq!(reply(Some(&owner), request), first);
        let Reply::Cursor(first) = first else {
            panic!()
        };
        assert_eq!(first.next, 8);
        assert!(
            matches!(ObservationRecord::decode(&first.record[..first.record_len as usize]),
            Ok(ObservationRecord::Gap(gap)) if gap.first_missing == 1 && gap.count == 8)
        );
        let Reply::Cursor(discovery) = reply(
            Some(&owner),
            Request::Cursor {
                request_id: 2,
                boot_id: 0,
                after_sequence: 0,
            },
        ) else {
            panic!()
        };
        assert_eq!(
            (
                discovery.boot_id,
                discovery.oldest,
                discovery.newest,
                discovery.record_len
            ),
            (9, 9, 40, 0)
        );
        assert_eq!(owner.recorder().stats(), before);
        let Reply::Profile(profile) = reply(
            Some(&owner),
            Request::Profile {
                request_id: 3,
                boot_id: 9,
                profile_id: 1,
            },
        ) else {
            panic!()
        };
        assert_eq!(
            selvage::decode_config_command(&profile.config).unwrap(),
            owner.profile(1).unwrap()
        );
    }
    #[test]
    fn wrong_boot_future_cursor_and_absent_owner_are_explicit() {
        let owner = OwnerObservations::new(9).unwrap();
        for (boot_id, after_sequence, expected) in [
            (8, 0, Status::WrongBoot),
            (9, 1, Status::FutureCursor),
            (0, 1, Status::InvalidRequest),
        ] {
            let Reply::Cursor(result) = reply(
                Some(&owner),
                Request::Cursor {
                    request_id: 1,
                    boot_id,
                    after_sequence,
                },
            ) else {
                panic!()
            };
            assert_eq!(result.status, expected);
            assert_eq!(result.record_len, 0);
        }
        let Reply::Cursor(result) = reply(
            None,
            Request::Cursor {
                request_id: 1,
                boot_id: 0,
                after_sequence: 0,
            },
        ) else {
            panic!()
        };
        assert_eq!(result.status, Status::Disabled);
    }
}
