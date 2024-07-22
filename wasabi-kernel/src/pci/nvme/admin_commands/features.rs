use bit_field::BitField;
use shared_derive::U8Enum;
use x86_64::structures::paging::PhysFrame;

use crate::pci::nvme::{
    generic_command::{PrpOrSgl, CDW0},
    CommonCommand,
};

use super::CommandOpcode;

/// See: NVM Express Base Spec: Figure 317: Feature Identifiers
#[repr(u8)]
#[derive(Clone, Copy, Debug, U8Enum)]
#[non_exhaustive]
pub enum FeatureIdentifier {
    NumberOfQueues = 0x7,
    IOCommandSet = 0x19,
}

#[non_exhaustive]
pub enum SetFeatureData {
    NumberOfQueues { sub_count: u16, comp_count: u16 },
    IOCommandSet { index: u32 },
}

impl SetFeatureData {
    pub fn identifier(&self) -> FeatureIdentifier {
        match self {
            SetFeatureData::NumberOfQueues {
                sub_count: _,
                comp_count: _,
            } => FeatureIdentifier::NumberOfQueues,
            SetFeatureData::IOCommandSet { index: _ } => FeatureIdentifier::IOCommandSet,
        }
    }
}

/// Create the [CommonCommand] data structure for a set features command
///
/// See: NVM Express Base Spec: 5.27
pub fn create_set_features_command(
    feature: SetFeatureData,
    data_frame: Option<PhysFrame>,
) -> CommonCommand {
    let mut cdw0 = CDW0::zero();
    cdw0.set_opcode(CommandOpcode::SetFeatures as u8);
    cdw0.set_prp_or_sgl(PrpOrSgl::Prp);

    let mut command = CommonCommand::default();
    command.dword0 = cdw0;

    let mut dword10 = 0;
    dword10.set_bits(0..=7, feature.identifier() as u32);

    let mut dword11 = 0;

    match feature {
        SetFeatureData::NumberOfQueues {
            sub_count,
            comp_count,
        } => {
            dword11.set_bits(0..=15, sub_count as u32);
            dword11.set_bits(16..=31, comp_count as u32)
        }
        SetFeatureData::IOCommandSet { index } => dword11.set_bits(0..=8, index),
    };

    command.dword10 = dword10;
    command.dword11 = dword11;

    if let Some(data_frame) = data_frame {
        command.data_ptr.prp_entry_1 = data_frame.start_address();
    }

    command
}
