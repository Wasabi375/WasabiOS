//! Utility for Cylinder Head Sector(CHS) Disk addressing

/// number of heads for CHS to LBA conversion
pub const HEADS: u32 = 255;

/// number of sectors for CHS to LBA conversion
pub const SECTORS: u32 = 63;

/// outdated CHS system to describe disk locations
#[allow(missing_docs)]
pub struct CHS {
    pub cylinder: u16,
    pub head: u8,
    pub sector: u8,
}

impl CHS {
    /// Calculate the lba for the CHS address
    ///
    /// assumes [HEAD] and [SECTORS] for the head and sector count
    pub fn to_lba(&self) -> Option<u64> {
        if self.cylinder > 1023
            || self.head as u32 >= HEADS
            || self.sector == 0
            || self.sector as u32 > SECTORS
        {
            return None; // invalid CHS
        }

        let lba = ((self.cylinder as u64 * HEADS as u64) + self.head as u64) * SECTORS as u64
            + (self.sector as u64 - 1);

        Some(lba)
    }

    /// Create a CHS address from an lba
    ///
    /// assumes [HEAD] and [SECTORS] for the head and sector count
    pub fn from_lba(lba: u64) -> Option<Self> {
        let temp = lba / SECTORS as u64;
        let sector: u8 = (lba % SECTORS as u64).try_into().ok()?;
        let sector = sector + 1; // sectors is 1 based
        let head = (temp % HEADS as u64).try_into().ok()?;
        let cylinder = (temp / HEADS as u64).try_into().ok()?;

        Some(CHS {
            cylinder,
            head,
            sector,
        })
    }

    /// Sentinel used in MBR to mean "unrepresentable CHS"
    pub const MAX: CHS = CHS {
        cylinder: 1023,
        head: 254,
        sector: 63,
    };

    /// Encode this CHS into the 3-byte MBR partition table format.
    pub fn encode(self) -> [u8; 3] {
        if self.cylinder > 1023 || self.head > 254 || self.sector == 0 || self.sector > 63 {
            return [0xFF, 0xFF, 0xFF]; // sentinel for "invalid/unrepresentable"
        }

        let b0 = self.head;
        let b1 = (self.sector & 0x3F) | (((self.cylinder >> 8) as u8) << 6);
        let b2 = (self.cylinder & 0xFF) as u8;

        [b0, b1, b2]
    }

    /// Decode CHS from the 3-byte MBR partition table format.
    pub fn decode(bytes: [u8; 3]) -> CHS {
        if bytes == [0xFF, 0xFF, 0xFF] {
            return CHS::MAX;
        }

        let head = bytes[0];
        let sector = bytes[1] & 0x3F;
        let cyl_hi = (bytes[1] & 0xC0) as u16 >> 6;
        let cyl_lo = bytes[2] as u16;
        let cylinder = (cyl_hi << 8) | cyl_lo;

        CHS {
            cylinder,
            head,
            sector,
        }
    }
}
