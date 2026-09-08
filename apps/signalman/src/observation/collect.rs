//! A finite, read-only collection pass over an exclusively held diagnostic link.
//! The caller chooses admission and page budgets and supplies host receipt time.
//! The initial newest sequence is the target; a later overwrite may return a
//! larger loss range. Raw gap records are retained unchanged, never clipped.
use super::{Admission, BUNDLE_VERSION, CarrierKind, ObservationBundle, ProfileEntry};
use radio_hand::observation::ObservationRecord;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};
use tulle::{
    observation_serial::ObservationClient,
    observation_wire::{CursorReply, Status},
};

pub struct Capture {
    pub bundle: ObservationBundle,
    pub initial: CursorReply,
    pub last: CursorReply,
    /// Reached the initial sequence ceiling, possibly through a later explicit
    /// overwrite gap. This does not certify complete radio coverage.
    pub reached_target: bool,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn accepted(status: Status) -> io::Result<()> {
    if status == Status::Ok {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "observation source replied {status:?}"
        )))
    }
}
fn bounds(reply: &CursorReply, boot: u64) -> io::Result<()> {
    accepted(reply.status)?;
    if reply.boot_id == 0
        || reply.boot_id != boot
        || reply.oldest > reply.newest
        || (reply.oldest == 0) != (reply.newest == 0)
        || reply.next > reply.newest
    {
        return Err(invalid("inconsistent observation snapshot"));
    }
    Ok(())
}

pub async fn capture<T: AsyncRead + AsyncWrite + Unpin>(
    client: &mut ObservationClient<T>,
    device: &[u8],
    carrier_label: &str,
    admission: Admission,
    max_pages: usize,
    mut received_unix_ms: impl FnMut() -> io::Result<u64>,
) -> io::Result<Capture> {
    let initial = client.cursor(0, 0).await?;
    bounds(&initial, initial.boot_id)?;
    if initial.record_len != 0 || initial.next != 0 {
        return Err(invalid("discovery returned unsolicited source records"));
    }
    let mut profiles = Vec::new();
    for id in 1..=initial.profile_count {
        let reply = client.profile(initial.boot_id, id).await?;
        accepted(reply.status)?;
        if reply.boot_id != initial.boot_id || reply.profile_id != id {
            return Err(invalid("profile identity changed during collection"));
        }
        let profile = tulle::decode_config_command(&reply.config)
            .map_err(|_| invalid("invalid source PHY definition"))?;
        let mut definition = b"selvage-config-v1:".to_vec();
        definition.extend_from_slice(&reply.config);
        profiles.push(ProfileEntry {
            id,
            version: 1,
            name: format!(
                "{} Hz / SF{}",
                profile.frequency_hz, profile.spreading_factor
            ),
            definition,
        });
    }
    let mut bundle = ObservationBundle::new(
        BUNDLE_VERSION,
        device,
        CarrierKind::LocalUsb,
        carrier_label,
        &profiles,
        admission,
    )
    .map_err(|error| invalid(&format!("{error:?}")))?;
    let mut last = initial;
    let mut after = 0;
    for _ in 0..max_pages {
        if after >= initial.newest {
            break;
        }
        let page = client.cursor(initial.boot_id, after).await?;
        let received = received_unix_ms()?;
        bounds(&page, initial.boot_id)?;
        if page.newest < last.newest || page.oldest < last.oldest || page.record_len == 0 {
            return Err(invalid(
                "source history disappeared without a boot change or gap",
            ));
        }
        let raw = &page.record[..usize::from(page.record_len)];
        let record =
            ObservationRecord::decode(raw).map_err(|_| invalid("invalid observation record"))?;
        let next = after
            .checked_add(1)
            .ok_or_else(|| invalid("source sequence exhausted"))?;
        let valid = match record {
            ObservationRecord::Event(event) => {
                event.boot_id == initial.boot_id
                    && event.sequence == next
                    && page.next == event.sequence
            }
            ObservationRecord::Gap(gap) => {
                gap.boot_id == initial.boot_id
                    && gap.first_missing == next
                    && page.next == gap.first_missing + (gap.count - 1)
            }
        };
        if !valid {
            return Err(invalid("record does not advance the requested cursor"));
        }
        bundle
            .admit(raw, received)
            .map_err(|error| invalid(&format!("{error:?}")))?;
        after = page.next;
        last = page;
    }
    Ok(Capture {
        bundle,
        initial,
        last,
        reached_target: after >= initial.newest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use radio_hand::observation::{
        ObservationRecord, RefusalReason, RequestKind, owner::OwnerObservations,
    };
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
    use tulle::observation_wire::{self as wire, Reply, Status};

    async fn serve(mut io: DuplexStream, owner: OwnerObservations) {
        loop {
            let mut command = [0; wire::MAX_OBSERVATION_COMMAND_LEN + 1];
            if io.read_exact(&mut command).await.is_err() {
                return;
            }
            assert_eq!(command[0], 0);
            let request = wire::decode_request(&command[1..command.len() - 1]).unwrap();
            let reply = radio_hand::observation::collection::reply(Some(&owner), request);
            let mut frame = [0; wire::MAX_OBSERVATION_REPLY_LEN];
            let length = wire::encode_reply(reply, &mut frame).unwrap();
            io.write_all(&frame[..length]).await.unwrap();
        }
    }

    fn admission() -> Admission {
        Admission {
            max_frames: 16,
            max_bytes: 16 * 1024,
        }
    }

    #[tokio::test]
    async fn capture_discovers_profiles_and_respects_finite_page_limit() {
        let profile = tulle::PhyProfile::meshtastic_long_fast(915_000_000);
        let mut owner = OwnerObservations::new(9).unwrap();
        owner.rx_damaged(10, profile);
        owner.rx_damaged(20, profile);
        let (host, board) = duplex(512);
        let task = tokio::spawn(serve(board, owner));
        let mut client =
            tulle::observation_serial::ObservationClient::new(host, Duration::from_secs(1))
                .unwrap();
        let result = capture(&mut client, b"node-a", "test", admission(), 1, || Ok(1000))
            .await
            .unwrap();
        assert_eq!(result.initial.profile_count, 1);
        assert_eq!(result.bundle.profiles().len(), 1);
        assert_eq!(result.bundle.entries().len(), 1);
        assert!(!result.reached_target);
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn capture_retains_overwrite_gap_and_rejects_wrong_boot() {
        let profile = tulle::PhyProfile::meshtastic_long_fast(915_000_000);
        let mut owner = OwnerObservations::new(9).unwrap();
        for time in 0..40 {
            owner.rx_damaged(time, profile);
        }
        let (host, board) = duplex(512);
        let task = tokio::spawn(serve(board, owner));
        let mut client =
            tulle::observation_serial::ObservationClient::new(host, Duration::from_secs(1))
                .unwrap();
        let result = capture(&mut client, b"node-a", "test", admission(), 1, || Ok(1000))
            .await
            .unwrap();
        assert!(
            matches!(result.bundle.entries()[0], super::super::BundleEntry::Record(ref frame)
            if matches!(ObservationRecord::decode(&frame.raw), Ok(ObservationRecord::Gap(gap))
                if gap.first_missing == 1 && gap.count == 8))
        );
        drop(client);
        task.await.unwrap();

        let (host, mut board) = duplex(512);
        let task = tokio::spawn(async move {
            let mut command = [0; wire::MAX_OBSERVATION_COMMAND_LEN + 1];
            board.read_exact(&mut command).await.unwrap();
            let discovery = Reply::Cursor(wire::CursorReply {
                request_id: 1,
                status: Status::Ok,
                boot_id: 9,
                oldest: 1,
                newest: 1,
                next: 0,
                recorded: 1,
                overwritten: 0,
                encode_failed: 0,
                profile_count: 0,
                record_len: 0,
                record: [0; 64],
            });
            let mut frame = [0; wire::MAX_OBSERVATION_REPLY_LEN];
            let length = wire::encode_reply(discovery, &mut frame).unwrap();
            board.write_all(&frame[..length]).await.unwrap();
            board.read_exact(&mut command).await.unwrap();
            let wrong_boot = Reply::Cursor(wire::CursorReply {
                request_id: 2,
                status: Status::WrongBoot,
                boot_id: 8,
                oldest: 0,
                newest: 0,
                next: 0,
                recorded: 0,
                overwritten: 0,
                encode_failed: 0,
                profile_count: 0,
                record_len: 0,
                record: [0; 64],
            });
            let length = wire::encode_reply(wrong_boot, &mut frame).unwrap();
            board.write_all(&frame[..length]).await.unwrap();
        });
        let mut client =
            tulle::observation_serial::ObservationClient::new(host, Duration::from_secs(1))
                .unwrap();
        let error = match capture(&mut client, b"node-a", "test", admission(), 1, || Ok(1000)).await
        {
            Ok(_) => panic!("wrong boot was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn capture_and_replay_owner_lifecycle_preserves_durations_and_points() {
        let profile = tulle::PhyProfile::meshtastic_long_fast(915_000_000);
        let mut owner = OwnerObservations::new(9).unwrap();
        owner.listening_started(10, 7, profile);
        owner.rx_captured(12, profile, 23, -70, 8);
        owner.listening_stopped(20, 7);
        owner.tx_started(30, profile, 5, 42);
        owner.tx_finished(40, 42);
        owner.listening_started(50, 7, profile);
        owner.refused(55, RequestKind::Transmit, RefusalReason::ChannelBusy, 43);

        let (host, board) = duplex(512);
        let task = tokio::spawn(serve(board, owner));
        let mut client =
            tulle::observation_serial::ObservationClient::new(host, Duration::from_secs(1))
                .unwrap();
        let result = capture(
            &mut client,
            b"node-lifecycle",
            "test",
            admission(),
            7,
            || Ok(1000),
        )
        .await
        .unwrap();
        assert!(result.reached_target);
        assert_eq!(result.bundle.profiles().len(), 1);
        let definition = &result.bundle.profiles()[0].definition;
        assert_eq!(
            tulle::decode_config_command(&definition[b"selvage-config-v1:".len()..]).unwrap(),
            profile
        );

        let timeline = super::super::replay(&result.bundle).unwrap();
        assert_eq!(timeline.summary.listening_ms.get(&1), Some(&10));
        assert_eq!(timeline.summary.transmit_ms, 10);
        assert_eq!(timeline.summary.captures, 1);
        assert_eq!(timeline.summary.refusals, 1);
        assert_eq!(timeline.summary.incomplete_intervals, 1);
        assert_eq!(timeline.intervals.len(), 3);
        assert_eq!(
            (timeline.intervals[0].start_ms, timeline.intervals[0].end_ms),
            (Some(10), Some(20))
        );
        assert_eq!(
            (timeline.intervals[1].start_ms, timeline.intervals[1].end_ms),
            (Some(30), Some(40))
        );
        assert_eq!(timeline.intervals[2].start_ms, Some(50));
        assert_eq!(timeline.intervals[2].end_ms, None);
        drop(client);
        task.await.unwrap();
    }
}
