use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u32);

        impl $name {
            pub const fn new(index: u32) -> Self {
                Self(index)
            }

            pub const fn raw(self) -> u32 {
                self.0
            }

            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

define_id!(SourceFileId);
define_id!(TypeId);
define_id!(StructId);
define_id!(FieldId);
define_id!(FunctionId);
define_id!(ParameterId);
define_id!(LocalId);
define_id!(StateId);
define_id!(InputId);
define_id!(OutputId);
define_id!(ControlOutputId);
define_id!(ParamId);
define_id!(BufferId);
define_id!(EventId);
define_id!(EventParamId);
define_id!(DelegateId);
define_id!(LogSiteId);
define_id!(ConstDataId);
