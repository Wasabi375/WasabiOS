//! This module contains functionality for encoding and sending IPIs

use bit_field::BitField;
use log::trace;
use shared_derive::U8Enum;
use volatile::{VolatilePtr, access::ReadWrite};

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

/// Represents a inter processor interrupt
///
/// This struct can be used to build messages that are sent to
/// other (and/or this) processors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipi {
    /// the delviery mode of the IPI
    pub mode: DeliveryMode,
    /// the destination for the IPI
    pub destination: Destination,
}

/// the bits used to encode the [DeliveryMode] in an IPI
const IPI_DELIVERY_MODE_BITS: usize = 8;
/// the bit used to encode the current delivery status of the IPI
///
/// this bit is set to 1 if there is an IPI pending.
/// New ipis should only be send when this is low.
pub(super) const IPI_STATUS_BIT: usize = 12;
/// the bit used to encode the assert value of an INIT IPI.
///
/// Should be set to 1 otherwise
const IPI_ASSERT_BIT: usize = 14;
/// the bits used to encode the IPI destination using a shorthand
const IPI_DEST_SHORT_BITS: usize = 18;
/// the bits used to encode the IPI destination if the shorthand is not used
const IPI_DEST_BITS: usize = 56 - 32;

impl Ipi {
    /// Encode the ipi into (low, high) u32s
    ///
    /// See Intel Vol 3. Chap 11.6.1
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
        low |= mode << IPI_DELIVERY_MODE_BITS;

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
        icr_low: VolatilePtr<u32, ReadWrite>,
        icr_high: VolatilePtr<u32, ReadWrite>,
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
    use testing::{KernelTestError, kernel_test, t_assert_eq};

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
            (0xc46ab, 0)
        );
        t_assert_eq!(
            Ipi {
                mode: DeliveryMode::Fixed(InterruptVector::Test),
                destination: Destination::AllIncludingSelf,
            }
            .encode(),
            (0x840ff, 0)
        );

        Ok(())
    }
}
