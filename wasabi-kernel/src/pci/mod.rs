//! PCI related structs and functions
//!
//! See  https://wiki.osdev.org/PCI for more information
//!

// TODO temp
#![allow(missing_docs)]

use alloc::vec::Vec;
use bit_field::BitField;
use shared_derive::U8Enum;
use x86_64::{
    instructions::port::{Port, PortGeneric, WriteOnlyAccess},
    structures::port::PortWrite,
};

#[allow(unused_imports)]
use crate::todo_warn;
#[allow(unused_imports)]
use log::{debug, info, trace, warn};

const CONFIG_PORT_ADR: u16 = 0xcf8;
const DATA_PORT_ADR: u16 = 0xcfc;

pub struct PCIAccess {
    config_port: PortGeneric<RegisterAddress, WriteOnlyAccess>,
    devices: Vec<Device>,
}

impl PCIAccess {
    /// check (device, vendor) for pci address
    pub fn check_vendor(&mut self, addr: Address) -> Option<(u16, u16)> {
        let register = RegisterAddress::from_addr(addr, 0, RegisterOffset::VendorID);

        let device_vendor = self.read32(register)?;

        Some((
            ((device_vendor >> 16) & 0xffff) as u16,
            (device_vendor & 0xffff) as u16,
        ))
    }

    pub fn read32(&mut self, register: RegisterAddress) -> Option<u32> {
        let value = unsafe {
            // Safety: writes to config are safe
            self.config_port.write(register);
            // Safety: read from data is safe
            register.data_port().read()
        };
        if value == !0 {
            None
        } else {
            Some(value)
        }
    }

    pub fn read16(&mut self, register: RegisterAddress) -> Option<u16> {
        let value = unsafe {
            // Safety: writes to config are safe
            self.config_port.write(register);
            // Safety: read from data is safe
            register.data_port16().read()
        };
        if value == !0 {
            None
        } else {
            Some(value)
        }
    }

    pub fn read8(&mut self, register: RegisterAddress) -> Option<u8> {
        let value = unsafe {
            // Safety: writes to config are safe
            self.config_port.write(register);
            // Safety: read from data is safe
            register.data_port8().read()
        };
        if value == !0 {
            None
        } else {
            Some(value)
        }
    }

    fn find_device_function_count(&mut self, addr: Address) -> u8 {
        let header = self
            .read8(RegisterAddress::from_addr(
                addr,
                0,
                RegisterOffset::HeaderType,
            ))
            .expect("failed to read pci header type");

        let multi_function = header.get_bit(7);
        if !multi_function {
            return 1;
        }
        let mut max = 0;
        while max < 8 {
            if self
                .read32(RegisterAddress::from_addr(
                    addr,
                    max,
                    RegisterOffset::VendorID,
                ))
                .is_none()
            {
                break;
            }
            max += 1;
        }
        if max == 0 {
            panic!("No supported functions found!");
        }
        return max + 1;
    }

    fn find_devices_on_bus(&mut self, bus: u8) {
        trace!("check pci bus {bus}");
        for device in 0..32 {
            if let Some(device) = Device::from_pci(self, Address { bus, device }) {
                info!("Device found: {device:#x?}");
                self.devices.push(device);
            }
        }
    }

    pub fn find_all_devices(&mut self) {
        if self.devices.len() > 0 {
            warn!("device list is already filled. Clearing existing list");
            self.devices.clear();
        }

        let Some(header) = self.read8(RegisterAddress::new(0, 0, 0, RegisterOffset::HeaderType))
        else {
            warn!("no PCI devices found!");
            return;
        };

        if header.get_bit(7) {
            debug!("pci multi function detected");
            for function in 0..8 {
                if self
                    .read16(RegisterAddress::new(
                        0,
                        0,
                        function,
                        RegisterOffset::VendorID,
                    ))
                    .is_none()
                {
                    break;
                }
                self.find_devices_on_bus(function);
            }
        } else {
            self.find_devices_on_bus(0);
        }
    }
}

/// PCI device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub device: u16,
    pub vendor: u16,
    pub address: Address,
    pub functions: u8,
    pub class: Class,
    pub revision: u8,
}

impl Device {
    pub fn from_pci(pci: &mut PCIAccess, address: Address) -> Option<Self> {
        let (device, vendor) = pci.check_vendor(address)?;

        let class_reg = pci
            .read32(RegisterAddress::from_addr(
                address,
                0,
                RegisterOffset::Revision,
            ))
            .unwrap();
        let class = class_reg.get_bits(24..=31) as u8;
        let subclass = class_reg.get_bits(16..=23) as u8;
        let prog_if = class_reg.get_bits(8..=15) as u8;
        let revision = class_reg.get_bits(0..=7) as u8;

        let class = Class::from_header(class, subclass, prog_if);

        let functions = pci.find_device_function_count(address);

        Some(Self {
            device,
            vendor,
            address,
            functions,
            class,
            revision,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Unclassified {
        vga_compatible: bool,
    },
    Storage(StorageSubclass),

    Unknown {
        class: u8,
        subclass: u8,
        prog_if: u8,
    },
}
impl Class {
    fn from_header(class: u8, subclass: u8, prog_if: u8) -> Self {
        match class {
            0x0 => {
                let vga_compatible = match subclass {
                    0x0 => false,
                    0x1 => true,
                    o => panic!("unexpected subclass for Unclassified class: {o}"),
                };
                Class::Unclassified { vga_compatible }
            }
            0x1 => Class::Storage(
                StorageSubclass::try_from(subclass).expect("unexpected storage subclass"),
            ),
            _ => Class::Unknown {
                class,
                subclass,
                prog_if,
            },
        }
    }
}

#[derive(U8Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StorageSubclass {
    Scsi = 0x0,
    Ide,
    Floppy,
    IpiBus,
    Raid,
    Ata,
    SerialAta,
    SErialAttachedScsi,
    NonVolatileMemory,
    Other = 0x80,
}

/// PCI address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Address {
    /// the bus of the device
    pub bus: u8,
    /// the device number. Max: 5 bits
    // TODO: enforce 5 bit maximum
    pub device: u8,
}

#[repr(u8)]
#[derive(U8Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterOffset {
    VendorID = 0x0,
    DeviceID = 0x2,

    Command = 0x4,
    Status = 0x6,

    Revision = 0x8,
    ProgIf = 0x9,
    Subclass = 0xa,
    Class = 0xb,

    CacheLineSize = 0xc,
    LatencyTimer = 0xd,
    HeaderType = 0xe,
    Bist = 0xf,
    // ...
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub offset: RegisterOffset,
}

impl RegisterAddress {
    pub fn new(bus: u8, device: u8, function: u8, offset: RegisterOffset) -> Self {
        Self {
            bus,
            device,
            function,
            offset,
        }
    }

    pub fn from_addr(addr: Address, function: u8, offset: RegisterOffset) -> Self {
        Self::new(addr.bus, addr.device, function, offset)
    }

    pub fn as_u32(&self) -> u32 {
        1u32 << 31
            | (self.bus as u32) << 16
            | (self.device as u32) << 11
            | (self.function as u32) << 8
            | self.offset as u8 as u32
    }

    pub fn config_register_offset(&self) -> u32 {
        self.as_u32() & !0b11
    }

    pub fn data_port(&self) -> Port<u32> {
        Port::new(DATA_PORT_ADR)
    }

    pub fn data_port16(&self) -> Port<u16> {
        let port = DATA_PORT_ADR + (self.offset as u8 & 2) as u16;
        Port::new(port)
    }

    pub fn data_port8(&self) -> Port<u8> {
        let port = DATA_PORT_ADR + (self.offset as u8 & 3) as u16;
        Port::new(port)
    }
}

impl From<u32> for RegisterAddress {
    fn from(value: u32) -> Self {
        let bus = value.get_bits(16..=23) as u8;
        let device = value.get_bits(11..=15) as u8;
        let function = value.get_bits(8..=10) as u8;
        let offset = (value.get_bits(0..=7) as u8).try_into().unwrap();

        Self {
            bus,
            device,
            function,
            offset,
        }
    }
}

impl PortWrite for RegisterAddress {
    unsafe fn write_to_port(port: u16, value: Self) {
        // Safety: same as function
        unsafe { u32::write_to_port(port, value.config_register_offset()) }
    }
}

/// Initialize PCI access
#[allow(unused_variables, unused_mut)] // TODO temp
pub fn init() {
    info!("Initializing pci");

    let mut pci = PCIAccess {
        config_port: PortGeneric::new(CONFIG_PORT_ADR),
        devices: Vec::new(),
    };

    pci.find_all_devices();

    todo!("store pci access in some kind of global");
}
