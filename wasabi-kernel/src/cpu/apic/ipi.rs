use bit_field::BitField;
use log::trace;
use shared_derive::U8Enum;
use volatile::{access::ReadWrite, Volatile};

use crate::cpu::interrupts::InterruptVector;

/// The type of IPI to be sent
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Interrupt decided based on vector.
    Fixed(InterruptVector),
    /// Same as [DeliveryMode::Fixed] but executing at lowest priority.
    ///
    /// This is not available on all processors.
    LowestPriority(InterruptVector),
    /// System Mangament Interrupt
    ///
    /// This is used by the bios/uefi and we can probably ignore this
    SMI,
    /// Delivers a Non-Maskable interrupt.
    NMI,
    /// a INIT interrupt to signal the processor to start.
    INIT,
    /// a StartUp interrupt.
    ///
    /// This tell the processor to perform the startup code
    /// at ipi vector after [DeliveryMode::Init].
    SIPI(u8),
}

/// The current status of the IPI register
#[derive(U8Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DeliveryStatus {
    /// The last IPI has been completed
    Idle = 0,
    /// APIC is currently sending IPI
    SendPending = 1,
}

impl From<bool> for DeliveryStatus {
    fn from(value: bool) -> Self {
        if value {
            DeliveryStatus::SendPending
        } else {
            DeliveryStatus::Idle
        }
    }
}

/// Shorthand to select processors to accept ipi
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Destination {
    /// No shorthand. Destination chosen based on destination field
    Target(u8),
    /// Self. Send the IPI to the sending processor.
    SelfOnly,
    /// Send the IPI to all processors including self.
    AllIncludingSelf,
    /// Send the IPI to all processors excluding self
    AllExcludingSelf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipi {
    pub mode: DeliveryMode,
    pub destination: Destination,
}

const IPI_MODE_BITS: usize = 8;
pub(super) const IPI_STATUS_BIT: usize = 12;
const IPI_ASSERT_BIT: usize = 14;
const IPI_DEST_SHORT_BITS: usize = 18;
const IPI_DEST_BITS: usize = 56 - 32;

impl Ipi {
    /// Encode the ipi into (low, high) u32s
    pub fn encode(&self) -> (u32, u32) {
        let mut low: u32 = 0;
        let mut high: u32 = 0;

        // should be set for all but deassert Inits
        low |= 1 << IPI_ASSERT_BIT;

        let mode = match self.mode {
            DeliveryMode::Fixed(v) => {
                low |= v as u8 as u32;
                0b000
            }
            DeliveryMode::LowestPriority(v) => {
                low |= v as u8 as u32;
                0b001
            }
            DeliveryMode::SMI => 0b010,
            DeliveryMode::NMI => 0b100,
            DeliveryMode::INIT => {
                // for deassert (not implemented) set assert bit to 0
                0b101
            }
            DeliveryMode::SIPI(v) => {
                low |= v as u32;
                0b110
            }
        };
        low |= mode << IPI_MODE_BITS;

        let dest_short = match self.destination {
            Destination::Target(dest) => {
                high = (dest as u32) << IPI_DEST_BITS;
                0b00
            }
            Destination::SelfOnly => 0b01,
            Destination::AllIncludingSelf => 0b10,
            Destination::AllExcludingSelf => 0b11,
        };
        low |= dest_short << IPI_DEST_SHORT_BITS;

        (low, high)
    }

    pub(super) fn send(
        &self,
        mut icr_low: Volatile<&mut u32, ReadWrite>,
        mut icr_high: Volatile<&mut u32, ReadWrite>,
    ) {
        // PERF: this should be fast enough that spin_loop or a failing option seems excessive
        loop {
            let status: DeliveryStatus = icr_low.read().get_bit(IPI_STATUS_BIT).into();
            if let DeliveryStatus::Idle = status {
                break;
            }
        }

        let (low, high) = self.encode();

        trace!("send ipi: 0x{:x}{:x}", high, low);

        if let Destination::Target(_) = self.destination {
            icr_high.write(high)
        }
        icr_low.write(low);
    }
}

#[cfg(feature = "test")]
mod test {
    use testing::{kernel_test, t_assert_eq, KernelTestError};

    use super::*;

    #[kernel_test]
    fn test_ipi_encodings() -> Result<(), KernelTestError> {
        t_assert_eq!(
            Ipi {
                mode: DeliveryMode::INIT,
                destination: Destination::AllExcludingSelf,
            }
            .encode(),
            (0x000C4500, 0)
        );
        t_assert_eq!(
            Ipi {
                mode: DeliveryMode::SIPI(0xab),
                destination: Destination::AllExcludingSelf,
            }
            .encode(),
            (0xc06ab, 0)
        );
        t_assert_eq!(
            Ipi {
                mode: DeliveryMode::Fixed(InterruptVector::Test),
                destination: Destination::AllIncludingSelf,
            }
            .encode(),
            (0x800ff, 0)
        );

        Ok(())
    }
}
