use linkboy::FlashEvent;

pub(super) fn render_event(event: FlashEvent) {
    match event {
        FlashEvent::Inspecting { device, package_id } => {
            println!("inspecting {device} with package {package_id}")
        }
        FlashEvent::WaitingForOwnerAction { message } => println!("owner action: {message}"),
        FlashEvent::EnteringBootloader => println!("entering bootloader"),
        FlashEvent::Rediscovering => println!("rediscovering device"),
        FlashEvent::Erasing => println!("erasing"),
        FlashEvent::Writing { written, total } => println!("writing {written}/{total}"),
        FlashEvent::VerifyingTransfer => println!("verifying transfer"),
        FlashEvent::Rebooting => println!("rebooting"),
        FlashEvent::VerifyingApplication => println!("verifying application"),
        FlashEvent::Complete { receipt } => {
            println!("complete");
            println!(
                "{}",
                receipt.to_json().unwrap_or_else(|error| error.to_string())
            );
        }
        FlashEvent::ManualCheckRequired { receipt } => {
            println!("manual check required");
            println!(
                "{}",
                receipt.to_json().unwrap_or_else(|error| error.to_string())
            );
        }
        FlashEvent::RecoveryRequired {
            facts,
            instructions,
            receipt,
        } => {
            println!("recovery required: {}", facts.detail);
            println!("before write: {}", instructions.before_write);
            println!("after failure: {}", instructions.after_failure);
            println!(
                "{}",
                receipt.to_json().unwrap_or_else(|error| error.to_string())
            );
        }
        FlashEvent::Refused { reasons } => {
            println!("refused:");
            for reason in reasons {
                println!("- {reason}");
            }
        }
    }
}
