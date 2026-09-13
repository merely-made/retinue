//! Link-only MC4a capacity image. Not the radio firmware and not installed by
//! the validation workflow. It links the same workload measured on the host,
//! with a fixed heap and explicit RAM reservations for later integration.
#![no_std]
#![no_main]

use embedded_alloc::LlffHeap;
use esp_backtrace as _;
use protocol_capacity_probe::Residents;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

const HEAP_BYTES: usize = 65_536;
#[global_allocator]
static HEAP: LlffHeap = LlffHeap::empty();

#[repr(C, align(16))]
struct HeapStorage([u8; HEAP_BYTES]);
static mut HEAP_STORAGE: HeapStorage = HeapStorage([0; HEAP_BYTES]);
static RESIDENTS: StaticCell<Residents> = StaticCell::new();

// Reserves coexist with the real protocol layouts; they are not a claim that
// a radio runtime or its peak stack is exercised by this image.
static mut RADIO_RUNTIME_RESERVE: [u8; 65_536] = [0; 65_536];
static mut QUEUE_STATE_RESERVE: [u8; 16_384] = [0; 16_384];

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    // SAFETY: single initialization before the first allocation. The aligned
    // static region remains valid and exclusively owned by HEAP forever.
    unsafe {
        HEAP.init(
            core::ptr::addr_of_mut!(HEAP_STORAGE).cast::<u8>() as usize,
            HEAP_BYTES,
        )
    };
    core::hint::black_box(core::ptr::addr_of_mut!(RADIO_RUNTIME_RESERVE));
    core::hint::black_box(core::ptr::addr_of_mut!(QUEUE_STATE_RESERVE));
    RESIDENTS.init_with(protocol_capacity_probe::workload);
    loop {
        core::hint::spin_loop();
    }
}
