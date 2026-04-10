#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TargetCpu {
    Host,
    Explicit(String),
}

impl Default for TargetCpu {
    fn default() -> Self {
        Self::Host
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TargetRelocMode {
    Default,
    Static,
    Pic,
    DynamicNoPic,
}

impl Default for TargetRelocMode {
    fn default() -> Self {
        Self::Default
    }
}

impl TargetRelocMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Static => "static",
            Self::Pic => "pic",
            Self::DynamicNoPic => "dynamic-no-pic",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TargetCodeModel {
    Default,
    Small,
    Kernel,
    Medium,
    Large,
}

impl Default for TargetCodeModel {
    fn default() -> Self {
        Self::Default
    }
}

impl TargetCodeModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Small => "small",
            Self::Kernel => "kernel",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TargetOptLevel {
    O0,
    O1,
    O2,
    O3,
}

impl Default for TargetOptLevel {
    fn default() -> Self {
        Self::O3
    }
}

impl TargetOptLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::O0 => "0",
            Self::O1 => "1",
            Self::O2 => "2",
            Self::O3 => "3",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct TargetConfig {
    pub triple: Option<String>,
    pub cpu: TargetCpu,
    pub features: Option<String>,
    pub reloc_model: TargetRelocMode,
    pub code_model: TargetCodeModel,
    pub opt_level: TargetOptLevel,
    pub abi_name: Option<String>,
}

impl TargetConfig {
    pub fn host() -> Self {
        Self::default()
    }

    pub fn for_triple(triple: impl Into<String>) -> Self {
        Self {
            triple: Some(triple.into()),
            cpu: TargetCpu::Explicit("generic".to_owned()),
            features: Some(String::new()),
            reloc_model: TargetRelocMode::Default,
            code_model: TargetCodeModel::Default,
            opt_level: TargetOptLevel::O3,
            abi_name: None,
        }
    }

    pub fn is_host_default(&self) -> bool {
        self.triple.is_none()
            && matches!(self.cpu, TargetCpu::Host)
            && self.features.is_none()
            && self.reloc_model == TargetRelocMode::Default
            && self.code_model == TargetCodeModel::Default
            && self.opt_level == TargetOptLevel::O3
            && self.abi_name.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct CodegenOptions {
    pub sample_rate: f32,
    pub block_size: usize,
    pub fast_math: bool,
    pub target: TargetConfig,
}

impl Default for CodegenOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            block_size: 512,
            fast_math: false,
            target: TargetConfig::host(),
        }
    }
}
